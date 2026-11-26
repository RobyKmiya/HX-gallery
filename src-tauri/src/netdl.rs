//! 网络画集下载：给一个作品页链接（exhentai / e-hentai），直接抓取全书图片、
//! 取回标签，打包成 CBZ 并导入画匣。
//!
//! 管理员 2026-11-25 要求：从左侧新增一个入口，输入链接就能把整本图集下载打包
//! 进库；标签要跟着进来，不能只有图。
//!
//! 实现取巧点：
//! - 复用系统自带 curl 抓页面/图片（与织机下载器同一条链路），走的是本机网络
//!   与系统代理，不需要额外引入 TLS 栈；
//! - cookie 直接问主窗 webview 要，用户在内置浏览器里登录过的站点无需二次登录；
//! - 解析用字符串查找而不是正则（站点结构简单，正则反而更难维护）。
//!
//! 站点结构（2026-11 实测）：
//! - 作品页：标题 `<h1 id="gn">`，标签在 `<div id="taglist">` 的表格里，
//!   每行 `<td class="tc">namespace:</td><td class="tc"><a>tag</a>...</td>`；
//!   缩略图分页 `?p=N`，每个缩略图链到 `/s/<hash>/<id>-<n>`；
//! - 图片页：原图在 `<img id="img" src="...">`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// 单本内部的图片并发上限：图站高峰期 8 路足够，再高只会触发限速。
const CONCURRENCY: usize = 8;
/// 全局图片并发闸门：多本并行时也别把 WebView2 / 系统代理打爆。
/// 2026-11-26 卡死根因之一：8 路 worker × 多本同时请求，WebView2 探针互等。
const GLOBAL_CONCURRENCY: usize = 12;
/// 单页缩略图数量（站点固定 20，用来算总页数）。
const THUMBS_PER_PAGE: usize = 20;

#[derive(Clone, Serialize, Default)]
pub struct NetDlStatus {
    pub running: bool,
    pub finished: bool,
    pub error: String,
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub title: String,
    pub tags: Vec<(String, String)>,
    pub path: String,
}

/// 一个下载任务：进度 + 各自的取消/暂停标志 + 在途 curl 子进程。
/// 每项独立，才能做到「启停单项」而互不影响。
struct DownloadTask {
    status: Mutex<NetDlStatus>,
    cancel: AtomicBool,
    pause: AtomicBool,
    /// 当前在途的 curl 子进程（图片下载），取消时 kill 掉立即生效。
    children: Mutex<Vec<std::process::Child>>,
}

impl DownloadTask {
    fn new() -> Self {
        Self {
            status: Mutex::new(NetDlStatus::default()),
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
            children: Mutex::new(Vec::new()),
        }
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        // 立刻 kill 所有在途 curl：老实现只置标志，worker 卡在 300s 的 curl 里
        // 根本看不到，表现就是「取消没反应、画匣卡死」。
        if let Ok(mut children) = self.children.lock() {
            for child in children.iter_mut() {
                let _ = child.kill();
            }
        }
        if let Ok(mut status) = self.status.lock() {
            status.stage = "正在取消…".to_string();
        }
    }
}

/// 全部任务。key = task_id（前端生成，uuid）。
static TASKS: Mutex<Option<HashMap<String, Arc<DownloadTask>>>> = Mutex::new(None);

/// 当前线程所属的任务。worker 线程开始时绑定，之后所有 set_status /
/// cookie_for / fetch_file 都自动认领自己的任务，不用改函数签名。
thread_local! {
    static CURRENT_TASK: RefCell<Option<Arc<DownloadTask>>> = const { RefCell::new(None) };
}

/// 全局图片并发闸门（信号量用 AtomicUsize 手写，避免引入依赖）。
static GATE_USED: AtomicUsize = AtomicUsize::new(0);

struct GatePermit;
impl Drop for GatePermit {
    fn drop(&mut self) {
        GATE_USED.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 拿一个全局并发名额；满了就等（每 50ms 重试，同时响应取消）。
fn acquire_gate(task: &DownloadTask) -> Result<GatePermit, String> {
    loop {
        if task.cancel.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        let used = GATE_USED.load(Ordering::SeqCst);
        if used < GLOBAL_CONCURRENCY
            && GATE_USED
                .compare_exchange(used, used + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return Ok(GatePermit);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 暂停时阻塞等待，直到继续 / 取消。只在每张图片之间检查，不打断在途请求。
fn wait_if_paused(task: &DownloadTask) -> Result<(), String> {
    while task.pause.load(Ordering::SeqCst) {
        if task.cancel.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if task.cancel.load(Ordering::SeqCst) {
        return Err("已取消".to_string());
    }
    Ok(())
}

/// 每个网络请求前的统一检查：暂停则等待，取消则报错退出。
fn wait_check() -> Result<(), String> {
    if let Some(task) = current_task() {
        wait_if_paused(&task)?;
    }
    Ok(())
}

fn current_task() -> Option<Arc<DownloadTask>> {
    CURRENT_TASK.with(|cell| cell.borrow().clone())
}

fn set_status(f: impl FnOnce(&mut NetDlStatus)) {
    let Some(task) = current_task() else { return };
    if let Ok(mut guard) = task.status.lock() {
        f(&mut guard);
    }
}

fn task_is_canceled() -> bool {
    current_task()
        .map(|t| t.cancel.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// 前端批量轮询：一次拿回所有任务快照（不再每本单独轮询）。
#[tauri::command]
pub fn net_dl_status_all() -> Vec<(String, NetDlStatus)> {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let Some(map) = guard.as_ref() else { return Vec::new() };
    map.iter()
        .filter_map(|(id, task)| {
            task.status.lock().ok().map(|s| (id.clone(), s.clone()))
        })
        .collect()
}

/// 兼容旧调用：返回当前线程任务的状态（前端迁移期用）。
#[tauri::command]
pub fn net_dl_status() -> NetDlStatus {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    guard
        .as_ref()
        .and_then(|map| map.values().find(|t| t.status.lock().map(|s| s.running).unwrap_or(false)))
        .and_then(|t| t.status.lock().ok().map(|s| s.clone()))
        .unwrap_or_default()
}

#[tauri::command]
pub fn net_dl_cancel_task(task_id: String) {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(task) = guard.as_ref().and_then(|map| map.get(&task_id)) {
        task.cancel();
    }
}

#[tauri::command]
pub fn net_dl_pause_task(task_id: String, paused: bool) {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(task) = guard.as_ref().and_then(|map| map.get(&task_id)) {
        task.pause.store(paused, Ordering::SeqCst);
        if let Ok(mut status) = task.status.lock() {
            status.stage = if paused {
                "已暂停".to_string()
            } else {
                "继续下载…".to_string()
            };
        }
    }
}

#[tauri::command]
pub fn net_dl_remove_task(task_id: String) {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(map) = guard.as_ref() {
        if let Some(task) = map.get(&task_id) {
            task.cancel(); // 移除前先杀掉在途请求
        }
    }
    if let Ok(mut guard) = TASKS.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&task_id);
        }
    }
}

/// 兼容旧的单全局取消（前端旧按钮 / 紧急停止用）：取消所有任务。
#[tauri::command]
pub fn net_dl_cancel() {
    let guard = match TASKS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(map) = guard.as_ref() {
        for task in map.values() {
            task.cancel();
        }
    }
}

/// 本机代理：**系统代理优先**，环境变量兜底。
///
/// 2026-11-26 事故：环境变量里残留 `HTTPS_PROXY=127.0.0.1:50714`（旧代理端口），
/// 而系统代理早已换成 `127.0.0.1:30808`。老实现优先读环境变量，于是所有 curl
/// 全打到死端口 —— 表现为「nhentai 织机必然出错、网络问题很大、全部超时」。
/// 浏览器能开就说明系统代理是活的，所以以它为准。
fn proxy_arg() -> Option<String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        if let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        {
            let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
            if enabled == 1 {
                if let Ok(server) = key.get_value::<String, _>("ProxyServer") {
                    if let Some(first) = server.split(';').next() {
                        let first = first.trim();
                        if !first.is_empty() {
                            let with_scheme = if first.starts_with("http") {
                                first.to_string()
                            } else {
                                format!("http://{first}")
                            };
                            return Some(with_scheme);
                        }
                    }
                }
            }
        }
    }
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn curl_base() -> std::process::Command {
    let mut command = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 不弹黑框：这轮修复的重点之一（网页标签并发请求曾闪满屏 curl 窗口）。
        command.creation_flags(0x0800_0000);
    }
    command
        .arg("-s")
        .arg("-L")
        // 最多跟 5 次重定向：未登录时 exhentai 会一路跳到登录页再打转，
        // 不设上限会以「退出码 47」失败且看不出真实原因。
        .arg("--max-redirs")
        .arg("5")
        .arg("--retry")
        .arg("2")
        .arg("--retry-delay")
        .arg("1")
        .arg("--retry-all-errors")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--max-time")
        .arg("120")
        .arg("-A")
        .arg(crate::gallery_dl::UA);
    command
}

/// Cookie 按 host 缓存。卡死根因：每张图都问 WebView2（alive_labels 持锁做 5s 探针），
/// 8 路 worker × 几百张图 = 上千次 WebView2 同步调用，主线程互等整窗卡死。
/// 现在一个 host 只问一次，结果缓存复用（登录态变了可清缓存）。
static COOKIE_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 清空 Cookie 缓存（用户重新登录后调用，避免沿用旧登录态）。
pub fn clear_cookie_cache() {
    if let Ok(mut guard) = COOKIE_CACHE.lock() {
        *guard = None;
    }
}

fn cookie_for(app: &AppHandle, url: &str) -> String {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return String::new();
    };
    let target_host = parsed.host_str().unwrap_or("").to_lowercase();

    // ① 缓存命中直接返回：热路径上彻底不碰 WebView2（防卡死的关键）。
    if let Ok(guard) = COOKIE_CACHE.lock() {
        if let Some(map) = guard.as_ref() {
            if let Some(cached) = map.get(&target_host) {
                return cached.clone();
            }
        }
    }

    // ② 落盘快照：DPAPI 加密存在库里，同样不经 WebView2。
    //    这是「保存登录态」后最稳的来源，优先于问活着的浏览器窗口。
    let mut result = crate::stored_cookie_for_host(app, &target_host).unwrap_or_default();

    // ③ 快照没有才问 WebView2（一次性，之后进缓存）。
    if result.is_empty() {
        result = cookie_from_webview(app, &parsed, &target_host);
    }

    // 写缓存（空值也缓存：没登录就别每张图反复问 WebView2）。
    if let Ok(mut guard) = COOKIE_CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(target_host, result.clone());
    }
    result
}

/// 从活着的浏览器 webview / 主窗取 cookie（慢路径，仅缓存未命中时调用一次）。
fn cookie_from_webview(app: &AppHandle, parsed: &tauri::Url, target_host: &str) -> String {
    let mut labels: Vec<String> = crate::browser::alive_labels();
    labels.sort_by_key(|label| {
        let matched = crate::browser::active_tab_url(label)
            .map(|current| current.to_lowercase().contains(&target_host))
            .unwrap_or(false);
        if matched {
            0
        } else {
            1
        }
    });

    for label in labels {
        let Ok(webview) = crate::browser::active_webview_of(&label) else {
            continue;
        };
        if let Ok(list) = webview.cookies_for_url(parsed.clone()) {
            if !list.is_empty() {
                return list
                    .iter()
                    .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
    }

    // 兜底：主窗（同一 WebView2 profile 时也能查到）。原来这里只要 cookies_for_url
    // 成功就 return，哪怕拿到的是空列表——那样下面的落盘快照永远没机会生效。
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(list) = window.cookies_for_url(parsed.clone()) {
            if !list.is_empty() {
                return list
                    .iter()
                    .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
    }

    String::new()
}

/// 抓一段文本（页面 HTML）。
fn fetch_text(app: &AppHandle, url: &str) -> Result<String, String> {
    wait_check()?;
    let mut command = curl_base();
    command.arg("--max-time").arg("45");
    let cookie = cookie_for(app, url);
    if !cookie.is_empty() {
        command.arg("-H").arg(format!("Cookie: {cookie}"));
    }
    if let Some(proxy) = proxy_arg() {
        command.arg("--proxy").arg(proxy);
    }
    command.arg(url);
    let output = command
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "拉取页面失败（curl 退出码 {:?}，28=超时，未走代理或站点连不上）：{url}",
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 是不是 Cloudflare 挑战页（curl 拿到这种就是被拦了）。
fn is_challenge(html: &str) -> bool {
    html.contains("Just a moment")
        || html.contains("cf-chl")
        || html.contains("challenge-platform")
}

/// 抓页面：先 curl（快），被 Cloudflare 拦住再交给内置浏览器（能执行挑战脚本）。
fn fetch_html_smart(app: &AppHandle, url: &str) -> Result<String, String> {
    if let Ok(text) = fetch_text(app, url) {
        if !is_challenge(&text) {
            return Ok(text);
        }
    }
    for label in crate::browser::alive_labels() {
        if let Ok(html) =
            crate::browser::load_html(&label, url, std::time::Duration::from_secs(25))
        {
            if !is_challenge(&html) && html.len() > 500 {
                return Ok(html);
            }
        }
    }
    Err("页面被 Cloudflare 拦住了：先在浏览器里打开一次该链接、把验证过一次，再回来下载。"
        .to_string())
}

/// 抓一个二进制文件到磁盘。
fn fetch_file(app: &AppHandle, url: &str, referer: &str, dest: &Path) -> Result<(), String> {
    wait_check()?;
    let mut command = curl_base();
    command
        .arg("--fail")
        .arg("--max-time")
        .arg("90")
        .arg("--retry")
        .arg("1");
    let cookie = cookie_for(app, url);
    if !cookie.is_empty() {
        command.arg("-H").arg(format!("Cookie: {cookie}"));
    }
    if !referer.is_empty() {
        command.arg("-e").arg(referer);
    }
    if let Some(proxy) = proxy_arg() {
        command.arg("--proxy").arg(proxy);
    }
    command
        .arg("-w")
        .arg("%{content_type}")
        .arg("-o")
        .arg(dest)
        .arg(url);
    // spawn 而非 output：把子进程登记到当前任务，取消时 kill 立即中断。
    // 轮询 try_wait 而不阻塞在 output()：这样取消能及时生效，也不会在
    // 主线程互等（老实现 Command::output 最长 300s 不可中断 = 卡死）。
    let mut child = command.spawn().map_err(|e| format!("调用 curl 失败：{e}"))?;
    let pid = child.id();
    if let Some(task) = current_task() {
        if let Ok(mut children) = task.children.lock() {
            children.push(child);
        }
    }
    // 轮到本任务的 child：它一直在表里，由 cancel() 负责 kill。
    loop {
        if task_is_canceled() {
            return Err("已取消".to_string());
        }
        let (done, found) = if let Some(task) = current_task() {
            if let Ok(mut children) = task.children.lock() {
                match children.iter_mut().find(|c| c.id() == pid) {
                    Some(c) => (c.try_wait().map_err(|e| format!("等待 curl 失败：{e}"))?, true),
                    None => (None, false), // 句柄不在表里（理论上只会被 cancel 清掉）
                }
            } else {
                (None, true)
            }
        } else {
            (None, true)
        };
        if done.is_some() || !found {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    // 请求完成，取回 child 读 stdout（content_type）。
    let mut child = if let Some(task) = current_task() {
        if let Ok(mut children) = task.children.lock() {
            children
                .iter()
                .position(|c| c.id() == pid)
                .map(|i| children.remove(i))
        } else {
            None
        }
    } else {
        None
    }
    .ok_or_else(|| "curl 子进程句柄丢失".to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("读取 curl 结果失败：{e}"))?;
    // 被取消时 child 已被 kill，这里不报具体错，让上层循环看到 CANCEL 自行退出。
    if task_is_canceled() {
        return Err("已取消".to_string());
    }
    if !output.status.success() {
        return Err(format!(
            "下载失败（curl 退出码 {:?}）：{url}",
            output.status.code()
        ));
    }
    // 图床超时/被验证时会返回 200 的 HTML 错误页，会被当成图片存下来
    // （管理员 2026-11-26：nhentai 下出来的是「Error: timeout」占位图）。
    let content_type = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();
    if !content_type.starts_with("image/") {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "下载到的不是图片（{content_type}），可能被验证页/错误页替换：{url}"
        ));
    }
    Ok(())
}

fn between<'a>(haystack: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = haystack.find(start)? + start.len();
    let rest = &haystack[from..];
    let to = rest.find(end)?;
    Some(&rest[..to])
}

fn html_unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// 作品标题。
fn parse_title(html: &str) -> String {
    let english = between(html, "<h1 id=\"gn\">", "</h1>")
        .map(|t| html_unescape(t.trim()))
        .unwrap_or_default();
    if !english.is_empty() {
        return english;
    }
    between(html, "<h1 id=\"gj\">", "</h1>")
        .map(|t| html_unescape(t.trim()))
        .unwrap_or_default()
}

/// 标签：命名空间 → 若干标签值。
fn parse_tags(html: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    // 锚点放宽：exhentai 的 taglist 常带 style/class，写死 `<div id="taglist">`
    // 会整块找不到（管理员 2026-11-26：标题、页数都对，tag 是空的）。
    let Some(pos) = html.find("id=\"taglist\"") else {
        return out;
    };
    let after_id = &html[pos..];
    let Some(gt) = after_id.find('>') else {
        return out;
    };
    let content = &after_id[gt + 1..];
    let Some(end) = content.find("</table>") else {
        return out;
    };
    let block = &content[..end];
    // 每行形如 <td class="tc">artist:</td><td class="tc"><a ...>name</a></td>
    for row in block.split("<tr>") {
        let Some(cell) = between(row, "<td class=\"tc\">", "</td>") else {
            continue;
        };
        let key = cell.trim();
        if key.is_empty() || !key.contains(':') {
            continue;
        }
        let namespace = key.trim_end_matches(':').to_string();
        let mut values: Vec<String> = Vec::new();
        let mut rest = row;
        while let Some(pos) = rest.find("</td><td class=\"tc\">") {
            rest = &rest[pos + "</td><td class=\"tc\">".len()..];
            let Some(cell_end) = rest.find("</td>") else { break };
            let segment = &rest[..cell_end];
            let mut scan = segment;
            while let Some(open) = scan.find('>') {
                let after = &scan[open + 1..];
                let Some(close) = after.find("</a>") else { break };
                let text = html_unescape(after[..close].trim());
                if !text.is_empty() && text != "=" {
                    values.push(text);
                }
                scan = &after[close..];
                if !scan.contains("<a ") {
                    break;
                }
                let Some(next) = scan.find("<a ") else { break };
                scan = &scan[next..];
            }
            if !values.is_empty() {
                break;
            }
        }
        if !values.is_empty() {
            out.entry(namespace).or_default().extend(values);
        }
    }
    out
}

/// 总页数（图片张数）。
fn parse_page_count(html: &str) -> usize {
    // <td class="tc">Length:</td><td class="tc">123 pages</td>
    if let Some(block) = between(html, ">Length:</td>", "</td>") {
        let digits: String = block.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<usize>() {
            return n;
        }
    }
    0
}

/// 收集所有缩略图指向的图片页链接（按页翻）。
fn collect_page_links(app: &AppHandle, gallery_url: &str, total: usize) -> Result<Vec<String>, String> {
    let mut links: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let pages = if total == 0 {
        1
    } else {
        (total + THUMBS_PER_PAGE - 1) / THUMBS_PER_PAGE
    };
    let base = gallery_url.trim_end_matches('/');
    for page in 0..pages {
        if task_is_canceled() {
            break;
        }
        set_status(|s| s.stage = format!("收集第 {}/{} 页缩略图…", page + 1, pages));
        let url = if page == 0 {
            base.to_string()
        } else {
            format!("{base}/?p={page}")
        };
        let html = fetch_text(app, &url)?;
        let mut cursor = 0usize;
        while let Some(pos) = html[cursor..].find("href=\"") {
            let start = cursor + pos + 6;
            let Some(end) = html[start..].find('"') else { break };
            let href = &html[start..start + end];
            cursor = start + end;
            if !href.contains("/s/") {
                continue;
            }
            let absolute = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("https://exhentai.org{}", if href.starts_with('/') { "" } else { "/" }).to_string()
                    + href
            };
            if seen.insert(absolute.clone()) {
                links.push(absolute);
            }
        }
    }
    Ok(links)
}

/// 图片页里的原图地址。
fn parse_image_url(html: &str) -> Option<String> {
    crate::gallery_dl::extract_image_url(html)
}

/// 从图片 URL 取不带扩展名的文件名（18comic 的 md5 要用，如 `00042`）。
fn image_file_stem(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let name = without_query.rsplit('/').next().unwrap_or(without_query);
    name.rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(name)
        .to_string()
}

fn extension_of(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    for ext in ["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"] {
        if lower.contains(&format!(".{ext}")) {
            return match ext {
                "jpeg" => "jpg",
                "jpg" => "jpg",
                "png" => "png",
                "webp" => "webp",
                "gif" => "gif",
                "bmp" => "bmp",
                _ => "avif",
            };
        }
    }
    "jpg"
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "网络画集".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn package_cbz(files: &[PathBuf], target: &Path) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};
    let file = std::fs::File::create(target).map_err(|e| format!("创建压缩包失败：{e}"))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, path) in files.iter().enumerate() {
        let name = format!(
            "{:04}.{}",
            index + 1,
            path.extension().and_then(|e| e.to_str()).unwrap_or("jpg")
        );
        zip.start_file(name, options)
            .map_err(|e| format!("写压缩包失败：{e}"))?;
        let mut source = std::fs::File::open(path).map_err(|e| format!("读图片失败：{e}"))?;
        std::io::copy(&mut source, &mut zip).map_err(|e| format!("写压缩包失败：{e}"))?;
    }
    zip.finish().map_err(|e| format!("收尾压缩包失败：{e}"))?;
    Ok(())
}

/// 预览：只抓作品页，拿标题/标签/页数给前端确认，不下载。
#[tauri::command]
pub async fn net_dl_preview(app: AppHandle, url: String) -> Result<NetDlStatus, String> {
    if url.contains("nhentai.net") {
        return preview_nhentai(&app, &url);
    }
    let html = fetch_text(&app, &url)?;
    let title = parse_title(&html);
    let tags = parse_tags(&html);
    let total = parse_page_count(&html);
    let flat: Vec<(String, String)> = tags
        .iter()
        .flat_map(|(namespace, values)| {
            values
                .iter()
                .map(move |value| (namespace.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(NetDlStatus {
        running: false,
        finished: false,
        error: String::new(),
        stage: if title.is_empty() {
            "已解析（没识别到标题，可能是登录态失效）".to_string()
        } else {
            format!("已解析：{total} 页")
        },
        done: 0,
        total,
        title,
        tags: flat,
        path: String::new(),
    })
}

/// 取 nhentai v2 作品 JSON。
///
/// 2026-11-26 根因：旧接口 `/api/gallery/{id}` 已下线，只回一句
/// "Use new API https://nhentai.net/api/v2/docs"（JSON 解析失败 → 回退 HTML）；
/// 而作品页改成 JS 渲染，HTML 里没有 data-src/i.nhentai.net，回退也失败 →
/// 管理员看到「nhentai 页面里没找到图片」。新版走 `/api/v2/galleries/{id}`，
/// 且 `pages[].path` 直接给出图片路径，不再依赖 HTML 抓取。
fn fetch_nhentai_v2(app: &AppHandle, id: &str) -> Result<serde_json::Value, String> {
    let raw = fetch_text(app, &format!("https://nhentai.net/api/v2/galleries/{id}"))?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        format!("nhentai v2 接口返回不是 JSON（可能被验证页拦截）：{e}；原文：{}", &raw[..raw.len().min(120)])
    })?;
    if value.get("error").is_some() {
        return Err(format!(
            "nhentai 作品 {id} 不存在或已删除：{}",
            value["error"].as_str().unwrap_or("")
        ));
    }
    Ok(value)
}

/// v2 的 pages[].path 形如 galleries/4354295/1.webp，拼图床域名取原图。
fn nhentai_v2_image_urls(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(pages) = value["pages"].as_array() {
        for page in pages {
            let path = page["path"].as_str().unwrap_or("").trim_start_matches('/');
            if path.is_empty() {
                continue;
            }
            let ext = page["path"].as_str().and_then(|p| p.rsplit('.').next()).unwrap_or("jpg");
            let base = path.strip_suffix(ext).unwrap_or(path).trim_end_matches('.');
            // 缩略图路径（.../1t.webp）去掉 t 后缀拿原图，兼容站点偶尔回缩略图。
            let original = if base.ends_with('t')
                && base
                    .rsplit('/')
                    .next()
                    .map(|s| s[..s.len() - 1].chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false)
            {
                let trimmed = &base[..base.len() - 1];
                format!("{trimmed}.{ext}")
            } else {
                path.to_string()
            };
            let url = if original.starts_with("http") {
                original
            } else {
                format!("https://i.nhentai.net/{original}")
            };
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

fn preview_nhentai(app: &AppHandle, url: &str) -> Result<NetDlStatus, String> {
    let id = nhentai_gallery_id(url)
        .ok_or("链接里没找到作品 id（应形如 nhentai.net/g/615034/）".to_string())?;
    let value = fetch_nhentai_v2(app, &id)?;
    let title = value["title"]["pretty"]
        .as_str()
        .or_else(|| value["title"]["english"].as_str())
        .unwrap_or("")
        .to_string();
    let total = value["num_pages"].as_u64().unwrap_or(0) as usize;
    let mut tags: Vec<(String, String)> = Vec::new();
    if let Some(list) = value["tags"].as_array() {
        for tag in list {
            let kind = tag["type"].as_str().unwrap_or("tag").to_string();
            let name = tag["name"].as_str().unwrap_or("").to_string();
            if !name.is_empty() {
                tags.push((kind, name));
            }
        }
    }
    Ok(NetDlStatus {
        running: false,
        finished: false,
        error: String::new(),
        stage: if title.is_empty() {
            "已解析（没识别到标题，可能是登录态失效）".to_string()
        } else {
            format!("已解析：{total} 页")
        },
        done: 0,
        total,
        title,
        tags,
        path: String::new(),
    })
}

/// 开始下载整本并入库（多任务版：每项有自己的 task_id，可独立启停）。
/// 前端传 task_id（uuid）；同一 id 重复调用会先取消旧任务。
#[tauri::command]
pub async fn net_dl_start_task(app: AppHandle, task_id: String, url: String) -> Result<(), String> {
    let task = Arc::new(DownloadTask::new());
    {
        let mut guard = match TASKS.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let map = guard.get_or_insert_with(HashMap::new);
        // 同 id 已存在：先取消旧的（防重复下载）。
        if let Some(old) = map.get(&task_id) {
            old.cancel();
        }
        if let Ok(mut status) = task.status.lock() {
            *status = NetDlStatus {
                running: true,
                stage: "解析作品页…".to_string(),
                ..Default::default()
            };
        }
        map.insert(task_id.clone(), task.clone());
    }

    let handle = app.clone();
    let task_for_thread = task.clone();
    std::thread::spawn(move || {
        // 绑定当前线程的任务身份：之后所有 set_status / cookie_for / fetch_file
        // 都自动认领这一项，不用改几百处调用签名。
        CURRENT_TASK.with(|cell| *cell.borrow_mut() = Some(task_for_thread.clone()));
        let result = run_download(&handle, &url);
        match result {
            Ok(path) => set_status(|s| {
                s.running = false;
                s.finished = true;
                let canceled = path.is_empty();
                s.path = if canceled { "已取消".to_string() } else { path };
                s.stage = if canceled {
                    "已取消".to_string()
                } else {
                    format!("完成：共 {} 张", s.done)
                };
            }),
            Err(error) => set_status(|s| {
                s.running = false;
                let canceled = error == "已取消";
                s.error = if canceled { String::new() } else { error.clone() };
                s.stage = if canceled {
                    "已取消".to_string()
                } else {
                    format!("失败：{error}")
                };
            }),
        }
    });
    Ok(())
}

/// 开始下载整本并入库（旧单任务兼容入口：用固定 id）。
#[tauri::command]
pub async fn net_dl_start(app: AppHandle, url: String) -> Result<(), String> {
    net_dl_start_task(app, "__legacy__".to_string(), url).await
}

fn run_download(app: &AppHandle, url: &str) -> Result<String, String> {
    // nhentai 走 API（structure 和 exhentai 完全不同），单独一条路。
    if url.contains("nhentai.net") {
        return run_nhentai_download(app, url);
    }
    if url.contains("18comic") {
        return run_18comic_download(app, url);
    }
    let html = fetch_text(app, url)?;
    let title = {
        let parsed = parse_title(&html);
        if parsed.is_empty() {
            "网络画集".to_string()
        } else {
            parsed
        }
    };
    let tags = parse_tags(&html);
    let total = parse_page_count(&html);
    let flat: Vec<(String, String)> = tags
        .iter()
        .flat_map(|(ns, values)| {
            values
                .iter()
                .map(|v| (ns.clone(), v.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    set_status(|s| {
        s.title = title.clone();
        s.tags = flat;
        s.total = total;
        s.stage = "收集缩略图…".to_string();
    });

    let links = collect_page_links(app, url, total)?;
    if links.is_empty() {
        return Err("没找到任何图片页，可能是登录态失效或链接不是作品页".to_string());
    }
    let count = links.len();
    set_status(|s| {
        s.total = count;
        s.stage = format!("并发 {CONCURRENCY} 路下载 {count} 张…");
    });

    let temp = std::env::temp_dir().join(format!(
        "xk-netdl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).map_err(|e| format!("建临时目录失败：{e}"))?;

    let cursor = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let files: Mutex<Vec<Option<PathBuf>>> = Mutex::new(vec![None; count]);
    let first_error: Mutex<Option<String>> = Mutex::new(None);

    let worker_task = current_task();
    // 共享变量取引用：move 闭包只搬引用 + task Arc，不搬所有权，
    // 否则 scope 外后续代码（打包/入库）就拿不到 files/temp 了。
    let (cursor, done) = (&cursor, &done);
    let (files, first_error) = (&files, &first_error);
    let (links, temp, app) = (&links, &temp, &app);
    std::thread::scope(|scope| {
        for _ in 0..CONCURRENCY.min(count) {
            let worker_task = worker_task.clone();
            scope.spawn(move || {
                // 子线程继承任务身份，set_status / fetch_file 才能认领这一项。
                CURRENT_TASK.with(|cell| *cell.borrow_mut() = worker_task);
                loop {
                if task_is_canceled() {
                    return;
                }
                let index = cursor.fetch_add(1, Ordering::SeqCst);
                if index >= count {
                    return;
                }
                let entry = &links[index];
                let result = (|| -> Result<PathBuf, String> {
                    // 全局闸门：多本并行时总并发不超 GLOBAL_CONCURRENCY。
                    let _gate = if let Some(task) = current_task() {
                        Some(acquire_gate(&task)?)
                    } else {
                        None
                    };
                    wait_check()?;
                    let page = fetch_text(app, entry)?;
                    let image = parse_image_url(&page)
                        .ok_or_else(|| format!("图片页里没找到原图：{entry}"))?;
                    let dest = temp.join(format!("{index:04}.{}", extension_of(&image)));
                    fetch_file(app, &image, entry, &dest)?;
                    Ok(dest)
                })();
                match result {
                    Ok(path) => {
                        if let Ok(mut guard) = files.lock() {
                            guard[index] = Some(path);
                        }
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        set_status(|s| {
                            s.done = n;
                            s.stage = format!("下载中 {n}/{count}");
                        });
                    }
                    Err(error) => {
                        // 取消 / 暂停退出不算真失败，别记进 first_error。
                        if error != "已取消" {
                            if let Ok(mut guard) = first_error.lock() {
                                if guard.is_none() {
                                    *guard = Some(error);
                                }
                            }
                        }
                    }
                }
                }
            });
        }
    });

    if task_is_canceled() {
        let _ = std::fs::remove_dir_all(&temp);
        set_status(|s| s.stage = "已取消".to_string());
        return Ok(String::new());
    }
    if let Ok(guard) = first_error.lock() {
        if let Some(error) = guard.as_ref() {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error.clone());
        }
    }

    let ordered: Vec<PathBuf> = files
        .lock()
        .map_err(|_| "收集结果失败：状态锁已损坏".to_string())?
        .iter()
        .flatten()
        .cloned()
        .collect();
    if ordered.is_empty() {
        let _ = std::fs::remove_dir_all(&temp);
        return Err("一张都没下载成功".to_string());
    }

    set_status(|s| s.stage = "打包 CBZ…".to_string());
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("缓存目录不可用：{e}"))?
        .join("netdl");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建缓存目录失败：{e}"))?;
    let target = dir.join(format!("{}.cbz", sanitize(&title)));
    package_cbz(&ordered, &target)?;
    let _ = std::fs::remove_dir_all(&temp);

    set_status(|s| s.stage = "导入画匣…".to_string());
    crate::d2::import_downloaded_meta(app, target.clone(), &title, &tags)?;
    Ok(target.to_string_lossy().to_string())
}

/// nhentai 有公开 JSON 接口，比解析 HTML 稳：一次拿到标题、标签和每页扩展名，
/// 图片直接从 i.nhentai.net 拉（地址由 media_id + 页号拼出来）。
fn run_nhentai_download(app: &AppHandle, url: &str) -> Result<String, String> {
    let id = nhentai_gallery_id(url)
        .ok_or("链接里没找到作品 id（应形如 nhentai.net/g/615034/）".to_string())?;
    set_status(|s| s.stage = format!("读取 nhentai 作品 {id}…"));
    // 旧接口 /api/gallery/{id} 已下线（返回 "Use new API ..."），改走 v2；
    // v2 的 pages[].path 直接给出图片路径，作品页 HTML 兜底已无意义。
    let value = fetch_nhentai_v2(app, &id)?;
    let urls = nhentai_v2_image_urls(&value);
    if urls.is_empty() {
        return Err("nhentai 接口没返回图片页（作品可能已删除或需要登录）".to_string());
    }
    let title = value["title"]["pretty"]
        .as_str()
        .or_else(|| value["title"]["english"].as_str())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("UNTITLE")
        .to_string();
    // 入库要 HashMap<namespace, Vec<tag>>；进度条用展平后的 Vec。
    let mut tag_map: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(list) = value["tags"].as_array() {
        for tag in list {
            let kind = tag["type"].as_str().unwrap_or("tag");
            let name = tag["name"].as_str().unwrap_or("");
            if !name.is_empty() {
                tag_map
                    .entry(kind.to_string())
                    .or_default()
                    .push(name.to_string());
            }
        }
    }
    let flat: Vec<(String, String)> = tag_map
        .iter()
        .flat_map(|(kind, names)| {
            names
                .iter()
                .map(move |name| (kind.clone(), name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    set_status(|s| {
        s.title = title.clone();
        s.tags = flat;
        s.total = urls.len();
        s.stage = "准备下载…".to_string();
    });

    download_and_package(app, urls, &title, &tag_map, url, None)
}

fn nhentai_gallery_id(url: &str) -> Option<String> {
    let marker = "/g/";
    let start = url.find(marker)? + marker.len();
    let rest = &url[start..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// 接口不可用时的回退：直接解析 nhentai 作品页 HTML。
fn run_nhentai_from_html(app: &AppHandle, page_url: &str) -> Result<String, String> {
    set_status(|s| s.stage = "接口被拦，改解析作品页…".to_string());
    let html = fetch_html_smart(app, page_url)?;
    let title = nhentai_title_from_html(&html).unwrap_or_else(|| "UNTITLE".to_string());
    let tag_map = nhentai_tags_from_html(&html);
    let urls = nhentai_image_urls(&html);
    if urls.is_empty() {
        return Err("nhentai 页面里没找到图片（可能是验证页或页面结构变了）".to_string());
    }
    let flat: Vec<(String, String)> = tag_map
        .iter()
        .flat_map(|(kind, names)| {
            names
                .iter()
                .map(move |name| (kind.clone(), name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    set_status(|s| {
        s.title = title.clone();
        s.tags = flat;
        s.total = urls.len();
    });
    download_and_package(app, urls, &title, &tag_map, page_url, None)
}

fn nhentai_title_from_html(html: &str) -> Option<String> {
    let pos = html.find("class=\"pretty\"")?;
    let after = &html[pos..];
    let gt = after.find('>')?;
    let rest = &after[gt + 1..];
    let end = rest.find("</span>")?;
    let text = html_unescape(rest[..end].trim());
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn nhentai_tags_from_html(html: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<a class=\"tag\"") {
        let after = &rest[pos..];
        let kind = after
            .find("data-type=\"")
            .and_then(|p| {
                let body = &after[p + 11..];
                body.find('"').map(|end| body[..end].to_string())
            })
            .unwrap_or_else(|| "tag".to_string());
        let name = after.find("<span class=\"name\">").and_then(|p| {
            let body = &after[p + "<span class=\"name\">".len()..];
            body.find("</span>")
                .map(|end| html_unescape(body[..end].trim()))
        });
        if let Some(name) = name {
            if !name.is_empty() {
                out.entry(kind).or_default().push(name);
            }
        }
        rest = &after[1..];
    }
    out
}

/// 从作品页拿每页图片地址：data-src 是缩略图（`1t.jpg`），去掉 `t` 就是原图。
fn nhentai_image_urls(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("data-src=\"") {
        let after = &rest[pos + "data-src=\"".len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        let mut src = after[..end].to_string();
        if src.starts_with("//") {
            src = format!("https:{src}");
        }
        if src.contains("i.nhentai.net") {
            if let Some(dot) = src.rfind('.') {
                let (base, ext) = src.split_at(dot);
                if base.ends_with('t') && base.len() > 1 {
                    src = format!("{}.{}", &base[..base.len() - 1], &ext[1..]);
                }
            }
            if !out.contains(&src) {
                out.push(src);
            }
        }
        rest = &after[end..];
    }
    out
}

/// 18comic：`/photo/{id}` 是单章（图片就在这页），`/album/{id}` 是整本（章节列表）。
fn run_18comic_download(app: &AppHandle, url: &str) -> Result<String, String> {
    let html = fetch_html_smart(app, url)?;
    let title = jm_title(&html).unwrap_or_else(|| "UNTITLE".to_string());
    let urls = jm_image_urls(&html);
    if urls.is_empty() {
        if url.contains("/album/") {
            return Err(
                "这是整本（album）页面：目前请先贴单章 /photo/ 链接下载，整本合并下一批做"
                    .to_string(),
            );
        }
        return Err(
            "18comic 页面里没找到图片：可能没登录（会员才能看图），或页面被验证页替换"
                .to_string(),
        );
    }
    let tags = jm_tags(&html);
    let flat: Vec<(String, String)> = tags
        .iter()
        .flat_map(|(kind, names)| {
            names
                .iter()
                .map(move |name| (kind.clone(), name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    // 18comic 对图片做了竖条混淆，浏览器端用 JS/CSS 拼回正常画面；curl 拿到的
    // 是混淆原图，必须按站点算法还原，否则下出来的漫画一条条错位（管理员截图）。
    let aid = jm_aid_from_url(url).unwrap_or(0);
    let scramble_id = jm_scramble_id(&html).unwrap_or(220980); // 站点兜底值
    set_status(|s| {
        s.title = title.clone();
        s.tags = flat;
        s.total = urls.len();
        if aid > 0 {
            s.stage = format!("下载 {} 张（含图片还原）…", urls.len());
        }
    });
    let jm = if aid > 0 {
        Some(JmDecode { scramble_id, aid })
    } else {
        None
    };
    download_and_package(app, urls, &title, &tags, url, jm.as_ref())
}

/// 从 /photo/{id} 或 /album/{id} 取章节/作品 id。
fn jm_aid_from_url(url: &str) -> Option<u64> {
    for marker in ["/photo/", "/album/"] {
        if let Some(pos) = url.find(marker) {
            let rest = &url[pos + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

/// 页面里的 `var scramble_id = 123456;`，取不到返回 None（调用方用站点默认值）。
fn jm_scramble_id(html: &str) -> Option<u64> {
    const KEY: &str = "scramble_id";
    let mut rest = html;
    while let Some(pos) = rest.find(KEY) {
        let after = &rest[pos + KEY.len()..];
        // 形如 ` = 268850;` 或 `=268850`，宽松取等号后的数字。
        if let Some(eq) = after.find('=') {
            let digits: String = after[eq + 1..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
        rest = &after[1..];
    }
    None
}

fn jm_title(html: &str) -> Option<String> {
    if let Some(pos) = html.find("<h1") {
        let after = &html[pos..];
        if let Some(gt) = after.find('>') {
            let rest = &after[gt + 1..];
            if let Some(end) = rest.find("</h1>") {
                let text = html_unescape(rest[..end].trim());
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    if let Some(pos) = html.find("<title>") {
        let rest = &html[pos + "<title>".len()..];
        if let Some(end) = rest.find("</title>") {
            let raw = html_unescape(rest[..end].trim());
            let title = raw.split(" - ").next().unwrap_or(&raw).trim().to_string();
            if !title.is_empty() {
                return Some(title);
            }
        }
    }
    None
}

/// 18comic 的图片地址：**按 `<img>` 在页面里的出现顺序**取，每个 img 只认一个属性
/// （data-original > data-src > src）。
///
/// 之前按属性分组提取（先所有 data-original 再所有 src），顺序被打乱，
/// 下出来的图页序是错的（管理员 2026-11-26：图片下载错位）。
fn jm_image_urls(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<img") {
        let after = &rest[pos..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = &after[..end];
        let mut candidate: Option<String> = None;
        for attr in ["data-original=", "data-src=", "src="] {
            let Some(p) = tag.find(attr) else {
                continue;
            };
            let value = &tag[p + attr.len()..];
            let value = value.trim_start_matches('"').trim_start_matches('\'');
            let Some(quote) = value.find(|c| c == '"' || c == '\'') else {
                break;
            };
            let mut src = value[..quote].to_string();
            if src.starts_with("//") {
                src = format!("https:{src}");
            }
            if src.contains("/media/photos/") {
                candidate = Some(src);
                break;
            }
        }
        if let Some(src) = candidate {
            if !out.contains(&src) {
                out.push(src);
            }
        }
        rest = &after[end..];
    }
    out
}

/// 18comic 的标签（`<a href="/tags/xxx">名字</a>`，也可能在 `class` 含 tag 的容器里）。
/// 18comic 的标签。
///
/// 2026-11-26 管理员反馈「下载的漫画 tag 看不到」。根因：老实现只找
/// `href="/tags/xxx"`（相册页格式），而单章 /photo/ 页的标签是
/// `<span itemprop="genre" data-type="tags">` 里的 `<a>`，或 `<meta name="keywords">`。
/// 参考 JMComic-Crawler-Python 的 pattern_html_album_tags / pattern_html_photo_tags，
/// 三路都试，取到即止，避免某一路结构变了就彻底没 tag。
fn jm_push_tag(out: &mut HashMap<String, Vec<String>>, raw: &str) {
    let name = html_unescape(raw.trim());
    if !name.is_empty() && name.len() < 40 && !name.contains('<') {
        let list = out.entry("tag".to_string()).or_default();
        if !list.contains(&name) {
            list.push(name);
        }
    }
}

fn jm_tags(html: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();

    // ① 新版 / 整本页：<span ... data-type="tags"> 里的一堆 <a>文字</a>。
    let mut rest = html;
    while let Some(pos) = rest.find("data-type=\"tags\"") {
        let after = &rest[pos..];
        // 找到这个 span 的结束，只在其内部取 <a>。
        let Some(span_end) = after.find("</span>") else { break };
        let block = &after[..span_end];
        let mut inner_rest = block;
        while let Some(a) = inner_rest.find("<a") {
            let a_after = &inner_rest[a..];
            let Some(gt) = a_after.find('>') else { break };
            let text_start = gt + 1;
            let Some(close) = a_after[text_start..].find("</a>") else { break };
            jm_push_tag(&mut out, &a_after[text_start..text_start + close]);
            inner_rest = &a_after[text_start + close + 4..];
        }
        rest = &after[span_end + 7..];
    }
    if !out.is_empty() {
        return out;
    }

    // ② 单章页：<meta name="keywords" content="a,b,c">（JMComic photo tags）。
    let mut rest = html;
    while let Some(pos) = rest.find("name=\"keywords\"") {
        let after = &rest[pos..];
        if let Some(cpos) = after.find("content=\"") {
            let value = &after[cpos + "content=\"".len()..];
            if let Some(end) = value.find('"') {
                for part in value[..end].split([',', '，']) {
                    jm_push_tag(&mut out, part);
                }
            }
        }
        rest = &after[pos + 1..];
    }
    if !out.is_empty() {
        return out;
    }

    // ③ 旧相册页兜底：href="/tags/xxx" 的文字。
    let mut rest = html;
    while let Some(pos) = rest.find("href=\"/tags/") {
        let after = &rest[pos..];
        let Some(gt) = after.find('>') else { break };
        let inner = &after[gt + 1..];
        let Some(end) = inner.find("</a>") else {
            rest = &after[1..];
            continue;
        };
        jm_push_tag(&mut out, &inner[..end]);
        rest = &after[end..];
    }
    out
}

/// 18comic 图片解码参数（站点把图片切成竖条打乱，见 jm_decode.rs）。
struct JmDecode {
    scramble_id: u64,
    aid: u64,
}

/// 下载一组图片并打包入库（nhentai API / HTML / 18comic 共用）。
/// `jm` 非空时，每张图下载完按 18comic 算法还原混淆。
fn download_and_package(
    app: &AppHandle,
    urls: Vec<String>,
    title: &str,
    tag_map: &HashMap<String, Vec<String>>,
    referer: &str,
    jm: Option<&JmDecode>,
) -> Result<String, String> {
    let count = urls.len();
    if count == 0 {
        return Err("没有可下载的图片".to_string());
    }
    let temp = std::env::temp_dir().join(format!(
        "xk-netdl-nh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).map_err(|e| format!("建临时目录失败：{e}"))?;

    let cursor = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let files: Mutex<Vec<Option<PathBuf>>> = Mutex::new(vec![None; count]);
    let first_error: Mutex<Option<String>> = Mutex::new(None);

    let worker_task = current_task();
    let (cursor, done) = (&cursor, &done);
    let (files, first_error) = (&files, &first_error);
    let (urls, temp, app, referer) = (&urls, &temp, &app, &referer);
    std::thread::scope(|scope| {
        for _ in 0..CONCURRENCY.min(count) {
            let worker_task = worker_task.clone();
            scope.spawn(move || {
                CURRENT_TASK.with(|cell| *cell.borrow_mut() = worker_task);
                loop {
                if task_is_canceled() {
                    return;
                }
                let index = cursor.fetch_add(1, Ordering::SeqCst);
                if index >= count {
                    return;
                }
                // 全局闸门 + 暂停等待：多本并行也不打爆 WebView2 / 代理。
                let gate = if let Some(task) = current_task() {
                    match acquire_gate(&task) {
                        Ok(g) => Some(g),
                        Err(_) => return,
                    }
                } else {
                    None
                };
                if let Err(_) = wait_check() {
                    return;
                }
                let image = &urls[index];
                let dest = temp.join(format!("{index:04}.{}", extension_of(image)));
                let fetch_result = fetch_file(app, image, referer, &dest);
                drop(gate); // 图片抓完立刻释放名额
                match fetch_result {
                    Ok(()) => {
                        // 18comic：按站点算法把混淆的竖条还原成正常图。
                        if let Some(jm) = jm {
                            let filename = image_file_stem(image);
                            let num = crate::jm_decode::segmentation_num(
                                jm.scramble_id,
                                jm.aid,
                                &filename,
                            );
                            if num > 0 {
                                if let Err(error) = crate::jm_decode::decode_image_file(&dest, num) {
                                    set_status(|s| {
                                        s.stage = format!("第 {} 张还原失败：{error}", index + 1)
                                    });
                                }
                            }
                        }
                        if let Ok(mut guard) = files.lock() {
                            guard[index] = Some(dest);
                        }
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        set_status(|s| {
                            s.done = n;
                            s.stage = format!("下载中 {n}/{count}");
                        });
                    }
                    Err(error) => {
                        // 取消 / 暂停退出不算真失败，别记进 first_error。
                        if error != "已取消" {
                            if let Ok(mut guard) = first_error.lock() {
                                if guard.is_none() {
                                    *guard = Some(error);
                                }
                            }
                        }
                    }
                }
                }
            });
        }
    });

    if task_is_canceled() {
        let _ = std::fs::remove_dir_all(&temp);
        set_status(|s| s.stage = "已取消".to_string());
        return Ok(String::new());
    }
    if let Ok(guard) = first_error.lock() {
        if let Some(error) = guard.as_ref() {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error.clone());
        }
    }

    let ordered: Vec<PathBuf> = files
        .lock()
        .map_err(|_| "收集结果失败：状态锁已损坏".to_string())?
        .iter()
        .flatten()
        .cloned()
        .collect();
    if ordered.is_empty() {
        let _ = std::fs::remove_dir_all(&temp);
        return Err("一张都没下载成功".to_string());
    }

    set_status(|s| s.stage = "打包 CBZ…".to_string());
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("缓存目录不可用：{e}"))?
        .join("netdl");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建缓存目录失败：{e}"))?;
    let target = dir.join(format!("{}.cbz", sanitize(title)));
    package_cbz(&ordered, &target)?;
    let _ = std::fs::remove_dir_all(&temp);
    set_status(|s| s.stage = "导入画匣…".to_string());
    crate::d2::import_downloaded_meta(app, target.clone(), title, tag_map)?;
    Ok(target.to_string_lossy().to_string())
}

#[cfg(test)]

mod tests {

    use super::*;



    #[test]

    fn jm_tags_reads_photo_meta_keywords() {

        let html = r#"<html><head><meta name="keywords" content="标签A,标签B,标签C"></head></html>"#;

        let tags = jm_tags(html);

        let got = tags.get("tag").unwrap();

        assert_eq!(got, &vec!["标签A".to_string(), "标签B".to_string(), "标签C".to_string()]);

    }



    #[test]

    fn jm_tags_reads_album_data_type_span() {

        let html = r#"<span itemprop="genre" data-type="tags"><a href="/tag/x">作者X</a><a href="/tag/y">标签Y</a></span>"#;

        let tags = jm_tags(html);

        let got = tags.get("tag").unwrap();

        assert!(got.contains(&"作者X".to_string()));

        assert!(got.contains(&"标签Y".to_string()));

    }



    #[test]

    fn jm_tags_falls_back_to_tags_href() {

        let html = r#"<a href="/tags/abc">旧标签</a>"#;

        let tags = jm_tags(html);

        assert!(tags.get("tag").unwrap().contains(&"旧标签".to_string()));

    }

}
