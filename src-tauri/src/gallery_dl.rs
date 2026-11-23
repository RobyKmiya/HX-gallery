//! 画廊整部下载引擎（下载面板的后端）。
//!
//! 与旧的 `d2::fetch_flow` 的分工：
//! - 旧流程把每张图读成 base64 再从页面回传，**串行**、慢，而且失败静默；
//! - 这里只借页面做两件事：拿作品信息和**全部**图片条目（分页收集），
//!   之后的翻页解析与图片下载都交给本进程的 curl **并发**完成，直接落盘，
//!   不再经过 base64。
//!
//! 支持：起止范围、并发数、打包成单个 CBZ、固定输出目录、取消、进度查询。

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

/// 下载与翻页都用这套 UA：不少图站会按 UA 拒绝 curl 默认值。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 并发上限。再高只会被图站限速，反而更容易触发风控。
const MAX_CONCURRENCY: usize = 16;

/// 面板开窗后的窗口 label（固定，单实例）。
pub const PANEL_LABEL: &str = "dl-panel";

/// 最近一次扫描出来的计划（面板点「下载」时直接复用，不重复收集）。
static PLAN: Mutex<Option<Plan>> = Mutex::new(None);

/// 进度快照：面板轮询读取。
static PROGRESS: Mutex<DlStatus> = Mutex::new(DlStatus {
    running: false,
    done: 0,
    total: 0,
    failed: 0,
    stage: String::new(),
    finished: false,
    path: String::new(),
    error: String::new(),
});

/// 每个站点的 cookie 头缓存（同一 host 只问 WebView 一次）。
static COOKIES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct Plan {
    label: String,
    url: String,
    title: String,
    site: String,
    entries: Vec<String>,
}

/// 面板轮询用的进度结构。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DlStatus {
    pub running: bool,
    /// 已处理张数（成功 + 失败）。
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    /// 一句话阶段说明。
    pub stage: String,
    pub finished: bool,
    /// 完成后的输出路径（CBZ 文件或图片目录）。
    pub path: String,
    pub error: String,
}

/// `dl_scan` 的返回：面板首屏需要的全部信息。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOut {
    pub url: String,
    pub title: String,
    pub site: String,
    pub count: usize,
    pub out_dir: String,
    pub default_concurrency: usize,
}

fn set_status(update: impl FnOnce(&mut DlStatus)) {
    if let Ok(mut guard) = PROGRESS.lock() {
        update(&mut guard);
    }
}

fn reset_status() {
    set_status(|status| {
        status.running = true;
        status.done = 0;
        status.total = 0;
        status.failed = 0;
        status.stage = "准备中…".to_string();
        status.finished = false;
        status.path.clear();
        status.error.clear();
    });
    if let Ok(mut guard) = COOKIES.lock() {
        *guard = Some(HashMap::new());
    }
    CANCEL.store(false, Ordering::SeqCst);
}

fn fail(message: String) {
    set_status(|status| {
        status.running = false;
        status.finished = true;
        status.error = message;
    });
}

// ── 命令：面板开合 ────────────────────────────────────────────

/// 打开下载面板（工具栏 ⬇ 与 D2 面板共用）。
pub fn open_panel(app: &AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(PANEL_LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    let handle = app.clone();
    app.run_on_main_thread(move || {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        if handle.get_webview_window(PANEL_LABEL).is_some() {
            return;
        }
        let built = WebviewWindowBuilder::new(
            &handle,
            PANEL_LABEL,
            WebviewUrl::App("download-panel.html".into()),
        )
        .title("画匣 · 下载")
        .inner_size(420.0, 620.0)
        .min_inner_size(380.0, 520.0)
        .always_on_top(true)
        .resizable(true)
        .build();
        if let Ok(window) = built {
            let _ = window.set_focus();
        }
    })
    .map_err(|error| format!("打开下载面板失败：{error}"))
}

#[tauri::command]
pub fn dl_open_panel(app: AppHandle) -> Result<(), String> {
    open_panel(&app)
}

// ── 命令：扫描 / 开始 / 状态 / 取消 ────────────────────────────

#[tauri::command]
pub async fn dl_scan(app: AppHandle) -> Result<ScanOut, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let gallery = crate::d2::scan_active(&app)?;
        let out_dir = default_out_dir();
        let _ = fs::create_dir_all(&out_dir);
        let scan = ScanOut {
            url: gallery.url.clone(),
            title: gallery.title.clone(),
            site: gallery.site.clone(),
            count: gallery.entries.len(),
            out_dir: out_dir.to_string_lossy().to_string(),
            default_concurrency: 8,
        };
        if let Ok(mut guard) = PLAN.lock() {
            *guard = Some(Plan {
                label: gallery.label,
                url: gallery.url,
                title: gallery.title,
                site: gallery.site,
                entries: gallery.entries,
            });
        }
        set_status(|status| {
            status.running = false;
            status.finished = false;
            status.total = scan.count;
            status.done = 0;
            status.failed = 0;
            status.stage = "已扫描".to_string();
            status.error.clear();
            status.path.clear();
        });
        Ok(scan)
    })
    .await
    .map_err(|error| format!("扫描失败：{error}"))?
}

#[tauri::command]
pub async fn dl_start(
    app: AppHandle,
    from: usize,
    to: usize,
    concurrency: usize,
    package_cbz: bool,
    import_to_library: bool,
    out_dir: String,
) -> Result<(), String> {
    let plan = PLAN
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or("还没有扫描：先让浏览器停在作品页，再打开下载面板")?;
    if PROGRESS
        .lock()
        .map(|status| status.running)
        .unwrap_or(false)
    {
        return Err("已经有一批在下载了，先等它跑完或取消".to_string());
    }

    reset_status();
    tauri::async_runtime::spawn_blocking(move || {
        let options = Opt {
            from,
            to,
            concurrency,
            package_cbz,
            import_to_library,
            out_dir,
        };
        if let Err(error) = run_download(app, plan, options) {
            fail(error);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn dl_status() -> DlStatus {
    PROGRESS
        .lock()
        .map(|status| status.clone())
        .unwrap_or(DlStatus {
            running: false,
            done: 0,
            total: 0,
            failed: 0,
            stage: String::new(),
            finished: false,
            path: String::new(),
            error: String::new(),
        })
}

#[tauri::command]
pub fn dl_cancel() -> Result<(), String> {
    CANCEL.store(true, Ordering::SeqCst);
    set_status(|status| status.stage = "正在取消…".to_string());
    Ok(())
}

#[tauri::command]
pub async fn dl_pick_folder(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        // 阻塞版本必须离开主线程调用，否则会死锁。
        match app.dialog().file().blocking_pick_folder() {
            Some(path) => path
                .into_path()
                .map(|path| path.to_string_lossy().to_string())
                .map_err(|error| format!("路径无效：{error}")),
            None => Err("没有选择目录".to_string()),
        }
    })
    .await
    .map_err(|error| format!("打开选择器失败：{error}"))?
}

#[tauri::command]
pub fn dl_open_folder(path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err("目录还不存在（下载完成后才有）".to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 目录用 explorer 直接打开；文件用 /select, 定位。
        let mut command = Command::new("explorer");
        if target.is_file() {
            command.raw_arg(format!("/select,\"{}\"", target.display()));
        } else {
            command.arg(target.as_os_str());
        }
        command
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|error| format!("打开资源管理器失败：{error}"))?;
    }
    Ok(())
}

// ── 下载主体 ─────────────────────────────────────────────────

struct Opt {
    from: usize,
    to: usize,
    concurrency: usize,
    package_cbz: bool,
    import_to_library: bool,
    out_dir: String,
}

fn run_download(app: AppHandle, plan: Plan, opts: Opt) -> Result<(), String> {
    let total = plan.entries.len();
    let from = opts.from.clamp(1, total);
    let to = opts.to.clamp(from, total);
    let picked: Vec<(usize, String)> = (from..=to)
        .map(|index| (index, plan.entries[index - 1].clone()))
        .collect();
    let count = picked.len();

    set_status(|status| {
        status.total = count;
        status.stage = format!("准备下载 {count} 张（第 {from}–{to} 张）");
    });

    // 浏览器窗口必须还在：翻页解析和 cookie 都要问它。
    let webview = crate::browser::active_webview_of(&plan.label)
        .map_err(|error| format!("浏览窗口用不了了：{error}"))?;

    let temp = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("获取缓存目录失败：{error}"))?
        .join("d2")
        .join("dl")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&temp).map_err(|error| format!("创建临时目录失败：{error}"))?;

    let queue = Arc::new(Mutex::new(VecDeque::from(picked)));
    let done = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let workers = opts.concurrency.clamp(1, MAX_CONCURRENCY).min(count.max(1));

    set_status(|status| status.stage = format!("并发 {workers} 路下载中…"));

    let mut handles = Vec::new();
    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let done = Arc::clone(&done);
        let failed = Arc::clone(&failed);
        let errors = Arc::clone(&errors);
        let webview = webview.clone();
        let temp = temp.clone();
        let gallery_url = plan.url.clone();
        handles.push(std::thread::spawn(move || loop {
            if CANCEL.load(Ordering::SeqCst) {
                return;
            }
            let item = queue.lock().ok().and_then(|mut queue| queue.pop_front());
            let Some((index, entry)) = item else { return };
            match fetch_one(&webview, &entry, &temp, index, &gallery_url) {
                Ok(()) => {}
                Err(error) => {
                    failed.fetch_add(1, Ordering::SeqCst);
                    if let Ok(mut list) = errors.lock() {
                        if list.len() < 5 {
                            list.push(format!("第 {index} 张：{error}"));
                        }
                    }
                }
            }
            let finished = done.fetch_add(1, Ordering::SeqCst) + 1;
            set_status(|status| {
                status.done = finished;
                status.failed = failed.load(Ordering::SeqCst);
                status.stage = format!("已下 {finished}/{count}");
            });
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }

    if CANCEL.load(Ordering::SeqCst) {
        let _ = fs::remove_dir_all(&temp);
        return Err("已取消".to_string());
    }

    let failed_count = failed.load(Ordering::SeqCst);
    let ok_count = count.saturating_sub(failed_count);
    if ok_count == 0 {
        let detail = errors
            .lock()
            .map(|list| list.join("；"))
            .unwrap_or_default();
        let _ = fs::remove_dir_all(&temp);
        return Err(format!(
            "{count} 张全部失败{}{}",
            if detail.is_empty() { "" } else { "：" },
            detail
        ));
    }

    // 落盘：拿到扩展名后按序号重命名。
    let mut files: Vec<PathBuf> = Vec::with_capacity(ok_count);
    for index in from..=to {
        let source = temp.join(format!("{index:04}.bin"));
        if !source.exists() {
            continue;
        }
        let ext = sniff_ext(&source);
        let target = temp.join(format!("{index:04}.{ext}"));
        if fs::rename(&source, &target).is_err() {
            continue;
        }
        files.push(target);
    }

    if files.is_empty() {
        let _ = fs::remove_dir_all(&temp);
        return Err("没有一张图片真正落盘（可能全被判成失败了）".to_string());
    }

    let out_dir = if opts.out_dir.trim().is_empty() {
        default_out_dir()
    } else {
        PathBuf::from(opts.out_dir.trim())
    };
    fs::create_dir_all(&out_dir).map_err(|error| format!("创建输出目录失败：{error}"))?;
    let safe_title = crate::d2::sanitize_file_name(&plan.title);
    let stamp = short_stamp();

    set_status(|status| status.stage = "正在整理输出…".to_string());

    let final_path = if opts.package_cbz {
        let target = unique_path(&out_dir.join(format!("{safe_title}-{stamp}.cbz")));
        package_cbz(&files, &target)?;
        target
    } else {
        let target = unique_path(&out_dir.join(format!("{safe_title}-{stamp}")));
        fs::create_dir_all(&target).map_err(|error| format!("创建目录失败：{error}"))?;
        for file in &files {
            if let Some(name) = file.file_name() {
                let _ = fs::copy(file, target.join(name));
            }
        }
        target
    };

    let _ = fs::remove_dir_all(&temp);

    if opts.import_to_library && opts.package_cbz {
        // import_downloaded 会把文件收进库（可能移动），所以先复制一份留给输出目录。
        let copy = app
            .path()
            .app_cache_dir()
            .map_err(|error| format!("获取缓存目录失败：{error}"))?
            .join("d2")
            .join("downloads")
            .join(format!(
                "{}-{}.cbz",
                safe_title,
                uuid::Uuid::new_v4()
            ));
        if let Some(parent) = copy.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if fs::copy(&final_path, &copy).is_ok() {
            crate::d2::import_downloaded(&app, copy);
        }
    }

    set_status(|status| {
        status.running = false;
        status.finished = true;
        status.path = final_path.to_string_lossy().to_string();
        status.stage = format!("完成：{ok_count} 张");
    });

    Ok(())
}

/// 下载单张：先拿图片直链（exhentai 要翻一层图片页），再把字节落到临时文件。
fn fetch_one(
    webview: &Webview,
    entry: &str,
    dir: &Path,
    index: usize,
    gallery_url: &str,
) -> Result<(), String> {
    let (image_url, referer) = match entry.strip_prefix("direct|") {
        // 直链条目的 referer 用作品页：部分图床会校验来源。
        Some(direct) => (direct.to_string(), gallery_url.to_string()),
        None => {
            let html = curl_text(webview, entry)?;
            let image = extract_image_url(&html)
                .ok_or_else(|| "图片页里没找到原图地址".to_string())?;
            (image, entry.to_string())
        }
    };
    let dest = dir.join(format!("{index:04}.bin"));
    curl_file(webview, &image_url, &referer, &dest)
}

fn curl_command() -> Command {
    let mut command = Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 不弹黑框：批量下载时每个请求都会闪一个控制台窗口。
        command.creation_flags(0x0800_0000);
    }
    command.arg("-s").arg("-L");
    command
}

fn curl_text(webview: &Webview, url: &str) -> Result<String, String> {
    let mut command = curl_command();
    command.arg("--max-time").arg("60").arg("-A").arg(UA);
    let cookie = cookie_header(webview, url);
    if !cookie.is_empty() {
        command.arg("-H").arg(format!("Cookie: {cookie}"));
    }
    command.arg(url);
    let output = command
        .output()
        .map_err(|error| format!("调用 curl 失败（系统里没有 curl？）：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "页面拉取失败（curl 退出码 {:?}）",
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn curl_file(webview: &Webview, url: &str, referer: &str, dest: &Path) -> Result<(), String> {
    let mut command = curl_command();
    command
        .arg("--fail")
        .arg("--max-time")
        .arg("300")
        .arg("-A")
        .arg(UA);
    let cookie = cookie_header(webview, url);
    if !cookie.is_empty() {
        command.arg("-H").arg(format!("Cookie: {cookie}"));
    }
    if !referer.trim().is_empty() {
        command.arg("-e").arg(referer);
    }
    command.arg("-o").arg(dest).arg(url);
    let output = command
        .output()
        .map_err(|error| format!("调用 curl 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "图片下载失败（curl 退出码 {:?}）",
            output.status.code()
        ));
    }
    let size = fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    if size == 0 {
        return Err("下载到的文件是空的".to_string());
    }
    Ok(())
}

/// 从 WebView 里问该 URL 对应的 cookie，并按 host 缓存。
fn cookie_header(webview: &Webview, url: &str) -> String {
    let host = tauri::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_string()))
        .unwrap_or_default();
    if host.is_empty() {
        return String::new();
    }
    if let Ok(mut guard) = COOKIES.lock() {
        if let Some(cache) = guard.as_mut() {
            if let Some(value) = cache.get(&host) {
                return value.clone();
            }
        }
    }
    let header = match tauri::Url::parse(url) {
        Ok(parsed) => match webview.cookies_for_url(parsed) {
            Ok(list) => list
                .iter()
                .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
                .collect::<Vec<_>>()
                .join("; "),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    };
    if let Ok(mut guard) = COOKIES.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.insert(host, header.clone());
        }
    }
    header
}

/// 从图片页 HTML 里抠出原图地址。
///
/// 先认 `id="original"` 的原图链接，再退回 `id="img"` 的 src / data-src。
fn extract_image_url(html: &str) -> Option<String> {
    if let Some(tag) = tag_with_id(html, "original") {
        if let Some(href) = attr_value(tag, "href") {
            return Some(unescape_html(&href));
        }
    }
    let tag = tag_with_id(html, "img")?;
    if let Some(value) = attr_value(tag, "data-src") {
        return Some(unescape_html(&value));
    }
    attr_value(tag, "src").map(|value| unescape_html(&value))
}

fn tag_with_id<'a>(html: &'a str, id: &str) -> Option<&'a str> {
    let needle = format!("id=\"{id}\"");
    let mut from = 0usize;
    while let Some(offset) = html[from..].find(&needle) {
        let hit = from + offset;
        let start = html[..hit].rfind('<')?;
        let end = html[hit..].find('>')? + hit;
        let tag = &html[start..=end];
        // 只接受标签本身，别把注释或脚本里的字符串当标签。
        if tag.starts_with('<') {
            return Some(tag);
        }
        from = end;
    }
    None
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        if let Some(offset) = tag.find(&needle) {
            let rest = &tag[offset + needle.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn unescape_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
}

/// 按文件头判断扩展名（图站经常把 jpg 塞进没有后缀的响应里）。
fn sniff_ext(path: &Path) -> &'static str {
    let mut buffer = [0u8; 12];
    let read = fs::File::open(path)
        .and_then(|mut file| {
            use std::io::Read;
            file.read(&mut buffer)
        })
        .unwrap_or(0);
    let head = &buffer[..read];
    if head.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if head.starts_with(b"GIF8") {
        "gif"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        "webp"
    } else {
        "jpg"
    }
}

fn package_cbz(files: &[PathBuf], target: &Path) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let file = fs::File::create(target).map_err(|error| format!("创建 CBZ 失败：{error}"))?;
    let mut writer = ZipWriter::new(file);
    // 图片本身已是压缩格式，Stored 更快且不会二次膨胀。
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, path) in files.iter().enumerate() {
        let name = format!(
            "{:04}.{}",
            index + 1,
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("jpg")
        );
        writer
            .start_file(name, options)
            .map_err(|error| format!("写入 CBZ 失败：{error}"))?;
        let bytes = fs::read(path).map_err(|error| format!("读取图片失败：{error}"))?;
        writer
            .write_all(&bytes)
            .map_err(|error| format!("写入 CBZ 失败：{error}"))?;
    }
    writer
        .finish()
        .map_err(|error| format!("收尾 CBZ 失败：{error}"))?;
    Ok(())
}

fn unique_path(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let parent = base.parent().unwrap_or(Path::new("."));
    let stem = base
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let ext = base.extension().and_then(|value| value.to_str());
    for index in 2..1000 {
        let name = match ext {
            Some(ext) => format!("{stem}-{index}.{ext}"),
            None => format!("{stem}-{index}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    base.to_path_buf()
}

fn short_stamp() -> String {
    // 用本地时间做后缀就够了：同一作品重复下载也不会互相覆盖。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
    format!("{}", now.as_secs() % 100_000_000)
}

/// 默认输出目录：图片库下的「画匣下载」。
fn default_out_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return PathBuf::from(profile).join("Pictures").join("画匣下载");
    }
    std::env::temp_dir().join("画匣下载")
}
