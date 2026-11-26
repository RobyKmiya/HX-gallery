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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// 并发上限：图站高峰期 8 路足够，再高只会触发限速。
const CONCURRENCY: usize = 8;
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

static STATUS: Mutex<Option<NetDlStatus>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn set_status(f: impl FnOnce(&mut NetDlStatus)) {
    let mut guard = match STATUS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let status = guard.get_or_insert_with(NetDlStatus::default);
    f(status);
}

/// 前端轮询用的进度快照。
#[tauri::command]
pub fn net_dl_status() -> NetDlStatus {
    let guard = match STATUS.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    guard.clone().unwrap_or_default()
}

#[tauri::command]
pub fn net_dl_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    set_status(|s| s.stage = "正在取消…".to_string());
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

fn cookie_for(app: &AppHandle, url: &str) -> String {
    // 登录态在**浏览器的 webview** 里，不在工作台主窗。以前只问主窗，
    // 结果 curl 裸奔：exhentai 重定向到登录页甚至打转（退出码 47），
    // nhentai/18comic 被当未登录（管理员 2026-11-26 的报错）。
    // 现在先问「当前打开目标站点」的浏览器 webview，再退回其它浏览器，最后才问主窗。
    let Ok(parsed) = tauri::Url::parse(url) else {
        return String::new();
    };
    let target_host = parsed.host_str().unwrap_or("").to_lowercase();

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

    // 兜底：主窗（同一 WebView2 profile 时也能查到）。
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(list) = window.cookies_for_url(parsed) {
            return list
                .iter()
                .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
                .collect::<Vec<_>>()
                .join("; ");
        }
    }
    String::new()
}

/// 抓一段文本（页面 HTML）。
fn fetch_text(app: &AppHandle, url: &str) -> Result<String, String> {
    let mut command = curl_base();
    command.arg("--max-time").arg("60");
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
    let mut command = curl_base();
    command
        .arg("--fail")
        .arg("--max-time")
        .arg("300")
        .arg("--retry")
        .arg("3");
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
    command.arg("-o").arg(dest).arg(url);
    let output = command
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "下载失败（curl 退出码 {:?}）：{url}",
            output.status.code()
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
        if CANCEL.load(Ordering::SeqCst) {
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

fn preview_nhentai(app: &AppHandle, url: &str) -> Result<NetDlStatus, String> {
    let id = nhentai_gallery_id(url)
        .ok_or("链接里没找到作品 id（应形如 nhentai.net/g/615034/）".to_string())?;
    let raw = fetch_text(app, &format!("https://nhentai.net/api/gallery/{id}"))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("nhentai 接口返回不是 JSON（可能被登录墙或验证页拦截）：{e}"))?;
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

/// 开始下载整本并入库。
#[tauri::command]
pub async fn net_dl_start(app: AppHandle, url: String) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    set_status(|s| {
        *s = NetDlStatus {
            running: true,
            stage: "解析作品页…".to_string(),
            ..Default::default()
        }
    });
    let handle = app.clone();
    std::thread::spawn(move || {
        let result = run_download(&handle, &url);
        match result {
            Ok(path) => set_status(|s| {
                s.running = false;
                s.finished = true;
                s.path = path;
                s.stage = format!("完成：共 {} 张", s.done);
            }),
            Err(error) => set_status(|s| {
                s.running = false;
                s.error = error.clone();
                s.stage = format!("失败：{error}");
            }),
        }
    });
    Ok(())
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

    std::thread::scope(|scope| {
        for _ in 0..CONCURRENCY.min(count) {
            scope.spawn(|| loop {
                if CANCEL.load(Ordering::SeqCst) {
                    return;
                }
                let index = cursor.fetch_add(1, Ordering::SeqCst);
                if index >= count {
                    return;
                }
                let entry = &links[index];
                let result = (|| -> Result<PathBuf, String> {
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
                        if let Ok(mut guard) = first_error.lock() {
                            if guard.is_none() {
                                *guard = Some(error);
                            }
                        }
                    }
                }
            });
        }
    });

    if CANCEL.load(Ordering::SeqCst) {
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
        .into_inner()
        .map_err(|e| format!("收集结果失败：{e}"))?
        .into_iter()
        .flatten()
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
    let api = format!("https://nhentai.net/api/gallery/{id}");
    let raw = fetch_text(app, &api)?;
    // 接口被 Cloudflare / 登录墙拦时会返回 HTML；退回解析作品页本体
    // （管理员 2026-11-26：接口返回不是 JSON）。
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return run_nhentai_from_html(app, url),
    };
    let media_id = value["media_id"]
        .as_str()
        .ok_or("nhentai 接口没有 media_id（可能未登录或作品已删除）")?
        .to_string();
    let pages = value["images"]["pages"]
        .as_array()
        .ok_or("nhentai 接口没有 pages")?
        .clone();
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
        s.total = pages.len();
        s.stage = "准备下载…".to_string();
    });

    let mut urls: Vec<String> = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let ext = match page["t"].as_str().unwrap_or("j") {
            "p" => "png",
            "g" => "gif",
            "w" => "webp",
            _ => "jpg",
        };
        urls.push(format!(
            "https://i.nhentai.net/galleries/{media_id}/{}.{ext}",
            index + 1
        ));
    }
    download_and_package(app, urls, &title, &tag_map, url)
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
    download_and_package(app, urls, &title, &tag_map, page_url)
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
    set_status(|s| {
        s.title = title.clone();
        s.total = urls.len();
    });
    download_and_package(app, urls, &title, &HashMap::new(), url)
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

/// 18comic 的图片在 data-original / data-src / src，统一过滤 `/media/photos/`。
fn jm_image_urls(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for attr in ["data-original=\"", "data-src=\"", "src=\""] {
        let mut rest = html;
        while let Some(pos) = rest.find(attr) {
            let after = &rest[pos + attr.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            let mut src = after[..end].to_string();
            if src.starts_with("//") {
                src = format!("https:{src}");
            }
            if src.contains("/media/photos/") && !out.contains(&src) {
                out.push(src);
            }
            rest = &after[end..];
        }
    }
    out
}

/// 下载一组图片并打包入库（nhentai API / HTML / 18comic 共用）。
fn download_and_package(
    app: &AppHandle,
    urls: Vec<String>,
    title: &str,
    tag_map: &HashMap<String, Vec<String>>,
    referer: &str,
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

    std::thread::scope(|scope| {
        for _ in 0..CONCURRENCY.min(count) {
            scope.spawn(|| loop {
                if CANCEL.load(Ordering::SeqCst) {
                    return;
                }
                let index = cursor.fetch_add(1, Ordering::SeqCst);
                if index >= count {
                    return;
                }
                let image = &urls[index];
                let dest = temp.join(format!("{index:04}.{}", extension_of(image)));
                match fetch_file(app, image, referer, &dest) {
                    Ok(()) => {
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
                        if let Ok(mut guard) = first_error.lock() {
                            if guard.is_none() {
                                *guard = Some(error);
                            }
                        }
                    }
                }
            });
        }
    });

    if CANCEL.load(Ordering::SeqCst) {
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
        .into_inner()
        .map_err(|e| format!("收集结果失败：{e}"))?
        .into_iter()
        .flatten()
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
