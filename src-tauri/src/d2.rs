//! D2 · 内建浏览器 + 一站式下载器
//!
//! 两条能力刻意解耦：
//! - **浏览**：一个受控 `WebviewWindow`，自带浏览器工具栏（地址栏 / 前进后退 /
//!   刷新 / 缩放），可访问任意站点，不设域名白名单——镜像、临时域名、登录跳转
//!   域名太多，白名单只会把人挡在登录页外面。
//! - **下载**：解析**整部作品**的每一个图片页并逐张取原图，打包成一个 CBZ
//!   后复用现有 importer 入库（早期实现只抓第一张，实际不可用）。
//!
//! ## 为什么是「后台线程 + 事件回报」而不是一个长命令
//!
//! 管理员 2026-11-19 反馈「粘贴链接后点击直接卡死」。命令必须**立即返回**，
//! 建窗、轮询解析、逐张下载、打包入库全在后台线程里跑，进度走
//! `d2://status`，结束走 `d2://done`。
//!
//! ## 页面脚本只解析、不落盘
//!
//! 注入脚本负责在页面里收集图片页 URL 与取字节（经 base64 回传），Rust 侧负责
//! 写盘与入库。页面拿不到任何画匣命令（浏览窗口没有 IPC 能力），安全边界不变。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::webview::Webview;
use tauri::{AppHandle, Emitter, Manager, State};
use zip::write::SimpleFileOptions;

use crate::models::{ImportOptions, ImportReport, TagDraft};
use crate::{importer, paths, AppState};

/// 整个抓取流程的上限。整部作品顺序下载几百张要不少时间，给足余量。
/// （窗口建窗超时已移交匣浏览器引擎 browser.rs。）
const FETCH_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// 单张图片的等待上限（从发起取字节到拿回结果）。
const GRAB_TIMEOUT: Duration = Duration::from_secs(60);
/// 页面打开后多久仍没有作品页结构，就提示用户检查登录/代理。
const NO_RESULT_HINT: Duration = Duration::from_secs(30);

static CANCELLED: AtomicBool = AtomicBool::new(false);
static BROWSER_LABEL: Mutex<Option<String>> = Mutex::new(None);

/// 注入到站点页面里的解析脚本。只做解析与取字节，结果写进 `window.__d2*`，
/// Rust 侧轮询读取（避开 WebView2 `eval_with_callback` 不等待 Promise 的限制）。
const D2_INIT_SCRIPT: &str = r###"
(function () {
  function clean(value) { return (value || "").trim(); }
  function abs(href) { try { return new URL(href, location.href).href; } catch (e) { return href; } }

  // 作品页元数据：标题 + 标签。
  window.__d2Meta = function () {
    var titles = document.querySelectorAll("#gd2 h1");
    var title = titles.length ? clean(titles[0].textContent) : "";
    var originTitle = titles.length > 1 ? clean(titles[1].textContent) : "";
    if (!title) title = originTitle || "UNTITLE";
    var tags = {};
    var category = clean((document.querySelector("#gdc > div") || {}).textContent);
    if (category) tags["category"] = [category];
    var uploader = clean((document.querySelector("#gdn > a") || {}).textContent);
    if (uploader) tags["uploader"] = [uploader];
    var trs = document.querySelectorAll("#taglist tr");
    for (var t = 0; t < trs.length; t++) {
      var cells = trs[t].children;
      if (cells.length < 2) continue;
      var key = clean(cells[0].textContent).replace(":", "");
      if (!key) continue;
      var values = [];
      var links = cells[1].querySelectorAll("a");
      for (var a = 0; a < links.length; a++) {
        var v = clean(links[a].textContent);
        if (v) values.push(v);
      }
      if (values.length) tags[key] = values;
    }
    return { title: title, tags: tags };
  };

  // 收集整部作品所有图片页 URL：本页缩略图 + 沿 #unext 逐页翻到底。
  window.__d2Collect = async function () {
    var seen = {};
    var urls = [];
    function grab(doc) {
      var links = doc.querySelectorAll("#gdt a");
      for (var i = 0; i < links.length; i++) {
        var href = links[i].href || "";
        if (href.indexOf("/s/") !== -1 && !seen[href]) { seen[href] = 1; urls.push(href); }
      }
    }
    var doc = document;
    var guard = 0;
    while (doc && guard < 500) {
      guard++;
      grab(doc);
      var next = doc.querySelector("#unext");
      var nextHref = next && next.href ? next.href : "";
      if (!nextHref || seen["#next#" + nextHref]) break;
      seen["#next#" + nextHref] = 1;
      var html;
      try {
        html = await fetch(nextHref, { credentials: "include" }).then(function (r) { return r.text(); });
      } catch (e) {
        break;
      }
      doc = new DOMParser().parseFromString(html, "text/html");
    }
    return urls;
  };

  // 取一张图片页的原图字节（base64）。失败时抛异常。
  window.__d2GrabOne = async function (pageUrl) {
    var html = await fetch(pageUrl, { credentials: "include" }).then(function (r) { return r.text(); });
    var doc = new DOMParser().parseFromString(html, "text/html");
    var original = (doc.querySelector("#image-resize-notice > a") || {}).href ||
                   (doc.querySelector("#original > a") || {}).href || "";
    var img = doc.querySelector("#img");
    var normal = (img && (img.getAttribute("data-src") || img.src)) || "";
    var imageUrl = original || normal;
    if (!imageUrl) throw new Error("图片页里没找到原图地址");
    var response = await fetch(abs(imageUrl), { credentials: "include" });
    if (!response.ok) throw new Error("图片下载失败，HTTP " + response.status);
    var blob = await response.blob();
    var base64 = await new Promise(function (resolve, reject) {
      var reader = new FileReader();
      reader.onload = function () { resolve(String(reader.result).split(",")[1] || ""); };
      reader.onerror = reject;
      reader.readAsDataURL(blob);
    });
    return { imageUrl: abs(imageUrl), mime: blob.type || "image/jpeg", base64: base64 };
  };
})();
"###;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct D2FetchReport {
    pub title: String,
    pub source_url: String,
    pub page_url: String,
    pub image_url: String,
    pub cbz_path: String,
    /// 成功下载并写入 CBZ 的张数。
    pub pages: u32,
    /// 失败（跳过）的张数。
    pub failed: u32,
    pub import_report: ImportReport,
}

/// `d2://done` 事件负载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct D2Done {
    pub ok: bool,
    pub stage: String,
    pub report: Option<D2FetchReport>,
    pub error: Option<String>,
}

/// `d2://status` 事件负载（过程中的一句话进度）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct D2Status {
    message: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MetaPayload {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GrabPayload {
    #[serde(default)]
    image_url: String,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    base64: String,
}

/// 收集阶段轮询到的页面状态。
#[derive(Debug, Deserialize, Default)]
struct CollectState {
    #[serde(default)]
    u: Option<Vec<String>>,
    #[serde(default)]
    e: Option<String>,
}

/// 单张下载阶段轮询到的页面状态。
#[derive(Debug, Deserialize, Default)]
struct GrabState {
    #[serde(default)]
    r: Option<GrabPayload>,
    #[serde(default)]
    e: Option<String>,
}

fn log_file(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_cache_dir().ok()?.join("d2");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("d2.log"))
}

fn log(app: &AppHandle, message: &str) {
    let Some(path) = log_file(app) else { return };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{stamp}] {message}");
    }
}

fn status(app: &AppHandle, message: &str) {
    log(app, message);
    let _ = app.emit("d2://status", D2Status { message: message.to_string() });
}

fn emit_done(app: &AppHandle, done: D2Done) {
    let _ = app.emit("d2://done", done);
}

pub fn sanitize_file_name(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|\r\n\t".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "UNTITLE".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn extension_for_mime(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "jpg",
    }
}

fn tag_drafts(tags: &HashMap<String, Vec<String>>) -> Vec<TagDraft> {
    let mut drafts = Vec::new();
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    for key in keys {
        for value in tags[key].iter().take(20) {
            if drafts.len() >= 60 {
                break;
            }
            if value.trim().is_empty() {
                continue;
            }
            drafts.push(TagDraft {
                name: value.trim().to_string(),
                category: Some(key.clone()),
            });
        }
    }
    drafts.push(TagDraft {
        name: "D2下载".to_string(),
        category: Some("来源".to_string()),
    });
    drafts
}

/// 同步执行一段 JS 并取回结果字符串（最多等 `wait`）。
///
/// `eval_with_callback` 不等待 Promise，所以页面侧统一用「先写全局变量、
/// 再轮询读取」的写法。调用方都在后台线程，不会阻塞主线程。
fn eval_json(window: &Webview, expression: &str, wait: Duration) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<String>();
    window
        .eval_with_callback(expression, move |value| {
            let _ = tx.send(value);
        })
        .map_err(|error| format!("读取页面结果失败：{error}"))?;

    let raw = rx.recv_timeout(wait).unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    Ok(serde_json::from_str::<String>(&raw).unwrap_or(raw))
}

/// 打开 D2 浏览窗口：全部窗口生命周期交给匣浏览器引擎（browser.rs）——
/// 工具栏、弹窗接管、下载上报、油猴内核、存活探针、幽灵窗防护都在引擎里。
/// 这里只保留 D2 自己的站点解析脚本与 WINDOW 句柄管理。
/// 打开 D2 浏览窗口：全部窗口生命周期交给匣浏览器引擎 v2（browser.rs）——
/// 多标签、独立工具栏、弹窗接管、下载上报、油猴内核、存活探针都在引擎里。
/// 这里只保留 D2 自己的站点解析脚本与浏览器 label 路由。
fn open_browser_window(
    app: &AppHandle,
    url: tauri::Url,
    label: String,
) -> Result<String, String> {
    let app_for_log = app.clone();
    // 窗口销毁/状态清理都在匣浏览器引擎内部完成（browser.rs）。
    let opened = crate::browser::open(
        app,
        url,
        label,
        crate::browser::BrowserOptions {
            title: "画匣 · 内建浏览器".to_string(),
            home: tauri::Url::parse("https://e-hentai.org/")
                .map_err(|error| format!("主页 URL 不合法: {error}"))?,
            extra_init_scripts: vec![D2_INIT_SCRIPT.to_string()],
            userscript_store: crate::userscripts::store_dir(app),
            size: (1180.0, 860.0),
            log: Some(std::sync::Arc::new(move |message| log(&app_for_log, message))),
        },
    )?;
    if let Ok(mut guard) = BROWSER_LABEL.lock() {
        *guard = Some(opened.clone());
    }
    Ok(opened)
}

/// 当前浏览器的窗口 label（没有浏览器在跑时为 None）。
fn current_label() -> Option<String> {
    BROWSER_LABEL.lock().ok()?.as_ref().cloned()
}

fn close_window() {
    if let Some(label) = current_label() {
        let _ = crate::browser::close_browser(&label);
    }
}

/// 等页面进入作品页（出现 `#gdt`）。期间对空白页做一次诊断提示。
fn wait_for_gallery(window: &Webview, app: &AppHandle, deadline: Instant) -> Result<(), String> {
    let started = Instant::now();
    let mut blank_checked = false;
    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        if Instant::now() > deadline {
            let hint = if !blank_checked {
                "作品页在限定时间内没有打开。请确认：① 系统代理已开启（WebView 走系统代理）；\
                 ② 已在该窗口里登录站点；③ 打开的是作品详情页（地址含 /g/）。"
            } else {
                "作品页打开了，但没解析出图片列表。请确认进入的是作品详情页。"
            };
            return Err(format!("抓取超时：{hint}"));
        }

        if let Ok(state) = eval_json(
            window,
            "JSON.stringify({g: !!document.querySelector('#gdt'), h: location.href, n: (document.body ? document.body.innerText.trim().length : 0)})",
            Duration::from_secs(3),
        ) {
            if state.contains("\"g\":true") {
                return Ok(());
            }
            if !blank_checked && started.elapsed() > Duration::from_secs(10) {
                blank_checked = true;
                if state.contains("poni=no") || state.contains("\"n\":0") {
                    status(
                        app,
                        "当前页面是空白。站点未登录一定白屏：请先用工具栏地址栏登录，登录成功后再回到作品页。",
                    );
                }
            }
        }

        if started.elapsed() > NO_RESULT_HINT {
            status(app, "仍在等待作品页……");
        }
        std::thread::sleep(Duration::from_millis(600));
    }
}

/// 触发页面侧收集并轮询结果。
fn collect_page_urls(window: &Webview, deadline: Instant) -> Result<Vec<String>, String> {
    window
        .eval(
            "window.__d2Urls=null;window.__d2UrlErr=null;\
             (window.__d2Collect?window.__d2Collect().then(function(u){window.__d2Urls=u})\
             .catch(function(e){window.__d2UrlErr=String((e&&e.message)||e)}):(window.__d2UrlErr='解析器未就绪'));",
        )
        .map_err(|error| format!("执行收集脚本失败：{error}"))?;

    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        if Instant::now() > deadline {
            return Err("收集图片列表超时".to_string());
        }
        let raw = eval_json(
            window,
            "JSON.stringify({u: window.__d2Urls, e: window.__d2UrlErr})",
            Duration::from_secs(3),
        )?;
        if !raw.trim().is_empty() {
            let state: CollectState =
                serde_json::from_str(&raw).map_err(|error| format!("解析收集结果失败：{error}"))?;
            if let Some(error) = state.e.as_deref().filter(|value| !value.is_empty()) {
                return Err(format!("收集图片列表失败：{error}"));
            }
            if let Some(urls) = state.u {
                return Ok(urls);
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// 取单张图片（原图字节）。
fn grab_one(window: &Webview, page_url: &str) -> Result<GrabPayload, String> {
    let encoded = serde_json::to_string(page_url).unwrap_or_else(|_| "\"\"".to_string());
    window
        .eval(&format!(
            "window.__d2Grab=null;window.__d2GrabErr=null;\
             (window.__d2GrabOne?window.__d2GrabOne({encoded}).then(function(r){{window.__d2Grab=r}})\
             .catch(function(e){{window.__d2GrabErr=String((e&&e.message)||e)}}):(window.__d2GrabErr='解析器未就绪'));"
        ))
        .map_err(|error| format!("执行下载脚本失败：{error}"))?;

    let deadline = Instant::now() + GRAB_TIMEOUT;
    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        if Instant::now() > deadline {
            return Err("下载超时".to_string());
        }
        let raw = eval_json(
            window,
            "JSON.stringify({r: window.__d2Grab, e: window.__d2GrabErr})",
            Duration::from_secs(3),
        )?;
        if !raw.trim().is_empty() {
            let state: GrabState =
                serde_json::from_str(&raw).map_err(|error| format!("解析下载结果失败：{error}"))?;
            if let Some(error) = state.e.as_deref().filter(|value| !value.is_empty()) {
                return Err(error.to_string());
            }
            if let Some(payload) = state.r {
                return Ok(payload);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn meta_of(window: &Webview) -> MetaPayload {
    eval_json(
        window,
        "JSON.stringify(window.__d2Meta ? window.__d2Meta() : {title:'',tags:{}})",
        Duration::from_secs(3),
    )
    .ok()
    .filter(|raw| !raw.trim().is_empty())
    .and_then(|raw| serde_json::from_str::<MetaPayload>(&raw).ok())
    .unwrap_or_default()
}

fn fetch_flow(
    app: &AppHandle,
    url: String,
    root: PathBuf,
    library_id: String,
    parsed: tauri::Url,
) -> Result<D2FetchReport, String> {
    // 抓取复用已打开浏览窗口的**激活标签**：用户正在看的作品页直接抓。
    // 浏览器没开就明确报错引导（多标签时代，抓谁要明确，不再后台偷偷开窗）。
    let label = current_label()
        .ok_or("浏览窗口还没打开——先在面板里点「打开」，进到作品页再抓取")?;
    if !crate::browser::is_alive(&label) {
        return Err("浏览窗口已无响应，请关掉后重新打开再抓取".to_string());
    }
    let window = crate::browser::active_webview_of(&label)?;
    let same = window
        .url()
        .map(|current| current.as_str() == parsed.as_str())
        .unwrap_or(false);
    if !same {
        let _ = window.navigate(parsed.clone());
    }
    log(app, "浏览窗口就绪");

    let started = Instant::now();
    let deadline = started + FETCH_TIMEOUT;
    status(app, "正在解析作品页…");
    wait_for_gallery(&window, app, deadline)?;

    let meta = meta_of(&window);
    status(app, "正在收集全部图片页…");
    let page_urls = collect_page_urls(&window, deadline)?;
    if page_urls.is_empty() {
        return Err(
            "没有解析到任何图片页。请确认打开的是作品详情页（地址含 /g/）且已登录。".to_string(),
        );
    }
    status(app, &format!("共 {} 张，开始逐张下载…", page_urls.len()));
    log(app, &format!("收集到 {} 张", page_urls.len()));

    let staging = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("获取缓存目录失败：{error}"))?
        .join("d2")
        .join("staging");
    fs::create_dir_all(&staging).map_err(|error| format!("创建暂存目录失败：{error}"))?;

    let title = if meta.title.trim().is_empty() {
        "UNTITLE".to_string()
    } else {
        meta.title.clone()
    };
    let cbz_path: PathBuf = staging.join(format!(
        "{}-{}.cbz",
        sanitize_file_name(&title),
        uuid::Uuid::new_v4()
    ));
    let file = fs::File::create(&cbz_path).map_err(|error| format!("创建 CBZ 失败：{error}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let total = page_urls.len();
    let mut pages = 0u32;
    let mut failed = 0u32;
    let mut first_image = String::new();
    let mut last_error = String::new();

    for (index, page_url) in page_urls.iter().enumerate() {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        if Instant::now() > deadline {
            last_error = "整体超时".to_string();
            break;
        }
        status(app, &format!("正在下载 {}/{}…", index + 1, total));
        match grab_one(&window, page_url) {
            Ok(payload) => match STANDARD.decode(payload.base64.as_bytes()) {
                Ok(bytes) if !bytes.is_empty() => {
                    let name = format!("{:04}.{}", index + 1, extension_for_mime(&payload.mime));
                    if writer.start_file(name, options).is_ok() && writer.write_all(&bytes).is_ok() {
                        if first_image.is_empty() {
                            first_image = payload.image_url;
                        }
                        pages += 1;
                    } else {
                        failed += 1;
                        last_error = "写入 CBZ 失败".to_string();
                    }
                }
                _ => {
                    failed += 1;
                    last_error = "图片数据为空或解码失败".to_string();
                }
            },
            Err(error) => {
                failed += 1;
                log(app, &format!("第 {} 张失败：{}", index + 1, error));
                last_error = error;
            }
        }
    }

    if pages == 0 {
        return Err(format!(
            "{} 张全部下载失败{}。若图片站点拒绝跨域读取，需要在设置里启用增强下载（待接入）。",
            total,
            if last_error.is_empty() {
                String::new()
            } else {
                format!("：{last_error}")
            }
        ));
    }

    writer
        .finish()
        .map_err(|error| format!("收尾 CBZ 失败：{error}"))?;
    status(app, "正在入库…");

    let covers_dir = paths::covers_dir(app, &library_id)?;
    let _guard = crate::try_maintenance()?;
    let options = ImportOptions {
        add_to_collection: None,
        preserve_structure: None,
        tags: tag_drafts(&meta.tags),
        path_tags: Vec::new(),
                passwords: Vec::new(),
    };
    let import_report = importer::import_blocking(
        app,
        &root,
        &covers_dir,
        vec![cbz_path.clone()],
        options,
    );

    Ok(D2FetchReport {
        title,
        source_url: url,
        page_url: page_urls.first().cloned().unwrap_or_default(),
        image_url: first_image,
        cbz_path: cbz_path.to_string_lossy().to_string(),
        pages,
        failed,
        import_report,
    })
}

/// 启动一次抓取。**立即返回**，进度与结果通过事件推送：
/// - `d2://status`：一句话进度
/// - `d2://done`：成功带 report，失败带 error
#[tauri::command]
pub fn d2_start(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "网址格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("请填写 http / https 网址".to_string());
    }
    // 这里限制的是「解析器能力」，不是浏览权限：任意站点都能用 d2_open 打开，
    // 但只有 exhentai / e-hentai 的详情页解析器写好了。
    let host = parsed.host_str().unwrap_or("");
    let supported = host.eq_ignore_ascii_case("exhentai.org")
        || host.eq_ignore_ascii_case("e-hentai.org");
    if !supported {
        return Err(format!(
            "抓取暂时只支持 exhentai / e-hentai（{host} 的解析器还没写）。\
             浏览不受限制——点上面的「打开」可以进任意站点。"
        ));
    }

    let (root, library_id) = {
        let inner = state.lock()?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (library.root.clone(), library.library_id.clone())
    };

    CANCELLED.store(false, Ordering::SeqCst);

    let worker = app.clone();
    std::thread::spawn(move || {
        let result = fetch_flow(&worker, url, root, library_id, parsed);
        match result {
            Ok(report) => {
                log(&worker, "完成");
                emit_done(
                    &worker,
                    D2Done {
                        ok: true,
                        stage: "done".to_string(),
                        report: Some(report),
                        error: None,
                    },
                );
            }
            Err(error) => {
                log(&worker, &format!("失败：{error}"));
                emit_done(
                    &worker,
                    D2Done {
                        ok: false,
                        stage: "error".to_string(),
                        report: None,
                        error: Some(error),
                    },
                );
            }
        }
    });

    Ok(())
}

/// 取消当前抓取：关掉浏览窗口，后台循环会退出。
#[tauri::command]
pub fn d2_cancel() -> Result<(), String> {
    CANCELLED.store(true, Ordering::SeqCst);
    close_window();
    Ok(())
}

/// 补全并校验用户输入的网址（允许省略 https://）。
fn normalize_url(raw: &str) -> Result<tauri::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("网址不能为空".to_string());
    }
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = tauri::Url::parse(&candidate).map_err(|_| "网址格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只接受 http / https 网址".to_string());
    }
    Ok(parsed)
}

/// 在已开浏览窗口的激活标签上执行一小段脚本。
fn with_window<F>(f: F) -> Result<(), String>
where
    F: FnOnce(&Webview) -> Result<(), String>,
{
    let label = current_label().ok_or("浏览窗口还没打开".to_string())?;
    let webview = crate::browser::active_webview_of(&label)?;
    f(&webview)
}

/// 打开 / 导航到一个网址用于**浏览**（不是抓取）。自带浏览器工具栏。
///
/// 不设域名白名单：站点地址千变万化，硬编码只会把用户挡在登录页外面。
/// 是否可抓取由解析器决定，与能否浏览解耦。
#[tauri::command]
pub fn d2_open(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = normalize_url(&url)?;

    // 浏览器还在且响应正常：激活标签直接导航（保留登录态与历史）。
    // 存活判定在引擎里用回调探针（browser::is_alive），冻住的尸体瞒不过它。
    if let Some(label) = current_label() {
        if crate::browser::is_alive(&label) {
            match crate::browser::navigate_active(&label, &parsed.to_string()) {
                Ok(()) => {
                    log(&app, &format!("导航到 {parsed}"));
                    return Ok(());
                }
                Err(error) => log(&app, &format!("导航失败，重开浏览器：{error}")),
            }
        } else {
            log(&app, "上次浏览窗口已无响应，重开一个");
        }
    }

    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    let app_for_window = app.clone();
    let target = parsed.to_string();
    // 建窗会阻塞，必须离开调用线程，命令本身立刻返回。
    std::thread::spawn(move || {
        status(&app_for_window, "正在打开浏览窗口…");
        match open_browser_window(&app_for_window, parsed, label) {
            Ok(opened) => {
                log(&app_for_window, &format!("窗口已创建：{target}（{opened}）"));
                status(&app_for_window, "窗口已打开，可以直接浏览。");
            }
            Err(error) => {
                log(&app_for_window, &format!("建窗失败：{error}"));
                let _ = app_for_window.emit("d2://error", error);
            }
        }
    });
    Ok(())
}

/// 读当前浏览窗口的地址（供「抓取当前页」一键使用，免去重新粘贴）。
#[tauri::command]
pub fn d2_current_url() -> Result<String, String> {
    let Some(label) = current_label() else {
        return Ok(String::new());
    };
    match crate::browser::active_webview_of(&label) {
        Ok(webview) => Ok(webview.url().map(|url| url.to_string()).unwrap_or_default()),
        Err(_) => Ok(String::new()),
    }
}

/// 后退。
#[tauri::command]
pub fn d2_back() -> Result<(), String> {
    let Some(label) = current_label() else {
        return Err("浏览窗口还没打开".to_string());
    };
    crate::browser::history_action(&label, "back")
}

/// 前进。
#[tauri::command]
pub fn d2_forward() -> Result<(), String> {
    let Some(label) = current_label() else {
        return Err("浏览窗口还没打开".to_string());
    };
    crate::browser::history_action(&label, "forward")
}

/// 刷新。
#[tauri::command]
pub fn d2_reload() -> Result<(), String> {
    let Some(label) = current_label() else {
        return Err("浏览窗口还没打开".to_string());
    };
    crate::browser::history_action(&label, "reload")
}

/// 用 Cookie 登录：把浏览器里复制出来的 Cookie 串写进内建浏览器的 cookie 库。
///
/// 为什么需要这条路（管理员 11-23）：exhentai 的登录流程（主站登录 → 跳
/// exhentai）在本机一直过不去，而且账号密码登录还会撞验证码。直接注入
/// `ipb_member_id` / `ipb_pass_hash` / `sk` 这些会话 cookie，等价于「已登录」。
///
/// 支持粘贴两种格式：
/// - 一整个 Cookie 请求头：`ipb_member_id=123; ipb_pass_hash=abc; sk=xyz`
/// - 开发工具里一条一条复制、每行一个：`ipb_member_id=123`
///
/// 同时写到 `.e-hentai.org` 与 `.exhentai.org`（两边共用同一套会话），
/// 写完后把打开的标签页刷新一遍。
#[tauri::command]
pub fn d2_set_cookies(app: tauri::AppHandle, cookie_header: String) -> Result<usize, String> {
    use tauri::Manager;

    let pairs = parse_cookie_pairs(&cookie_header);
    if pairs.is_empty() {
        return Err("没解析出任何 cookie：请粘贴 name=value 形式的内容".to_string());
    }

    let mut written = 0usize;
    let webviews = app.webviews();
    if webviews.is_empty() {
        return Err("浏览窗口还没打开：先点「打开浏览器」".to_string());
    }

    for (label, webview) in webviews {
        // 只往浏览器的标签页里写，不动主界面。
        if !label.contains("-tab-") {
            continue;
        }
        for (name, value) in &pairs {
            for domain in [".e-hentai.org", ".exhentai.org"] {
                let mut cookie = cookie::Cookie::new(name.clone(), value.clone());
                cookie.set_domain(domain);
                cookie.set_path("/");
                cookie.set_secure(true);
                cookie.set_http_only(true);
                if webview.set_cookie(cookie).is_ok() {
                    written += 1;
                }
            }
        }
        // 写完刷新当前页，让站点头像/权限立刻生效。
        if let Ok(mut url) = webview.url() {
            url.set_fragment(None);
            let _ = webview.navigate(url);
        }
    }

    if written == 0 {
        return Err("写入 cookie 失败：浏览窗口可能已经关掉了".to_string());
    }
    Ok(written)
}

/// 解析 Cookie 头：分号或换行分隔，取第一条 `=` 前后。
fn parse_cookie_pairs(raw: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for part in raw.split([';', '\n', '\r']) {
        let piece = part.trim();
        if piece.is_empty() {
            continue;
        }
        // 跳过属性行（Path=/; Domain=...; HttpOnly 之类）
        let lower = piece.to_ascii_lowercase();
        if lower.starts_with("path=")
            || lower.starts_with("domain=")
            || lower.starts_with("expires=")
            || lower.starts_with("max-age=")
            || lower == "httponly"
            || lower == "secure"
            || lower.starts_with("samesite=")
        {
            continue;
        }
        if let Some((name, value)) = piece.split_once('=') {
            let name = name.trim();
            let value = value.trim();
            if !name.is_empty() && !value.is_empty() {
                pairs.push((name.to_string(), value.to_string()));
            }
        }
    }
    pairs
}

/// 关掉浏览窗口（不影响主界面）。
#[tauri::command]
pub fn d2_close_window() -> Result<(), String> {
    close_window();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_illegal_filename_chars() {
        assert_eq!(sanitize_file_name("a/b:c*?\"<>|d"), "a_b_c______d");
        assert_eq!(sanitize_file_name("  中文标题  "), "中文标题");
        assert_eq!(sanitize_file_name("..."), "UNTITLE");
    }

    #[test]
    fn tag_drafts_flatten_categories_and_add_source() {
        let mut tags = HashMap::new();
        tags.insert("tag:女仆".to_string(), vec!["女仆".to_string(), "".to_string()]);
        tags.insert("category".to_string(), vec!["Manga".to_string()]);
        let drafts = tag_drafts(&tags);
        assert!(drafts.iter().any(|d| d.name == "女仆" && d.category.as_deref() == Some("tag:女仆")));
        assert!(drafts.iter().any(|d| d.name == "Manga" && d.category.as_deref() == Some("category")));
        assert!(drafts.iter().any(|d| d.name == "D2下载" && d.category.as_deref() == Some("来源")));
    }
}
