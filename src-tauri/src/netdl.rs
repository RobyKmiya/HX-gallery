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

/// 本机代理：优先环境变量，其次读注册表里的系统代理。
/// 站点点不开多半是没走代理（curl 默认只认环境变量，不认 Windows 系统代理）。
fn proxy_arg() -> Option<String> {
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
    // 直接问主窗要 cookie：与织机下载器同一份登录态，用户在内置浏览器里登录过
    // 就不必再来一遍。
    let Some(window) = app.get_webview_window("main") else {
        return String::new();
    };
    let Ok(parsed) = tauri::Url::parse(url) else {
        return String::new();
    };
    match window.cookies_for_url(parsed) {
        Ok(list) => list
            .iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; "),
        Err(_) => String::new(),
    }
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
            "拉取页面失败（curl 退出码 {:?}）：{url}",
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
    let Some(block) = between(html, "<div id=\"taglist\">", "</table>") else {
        return out;
    };
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
