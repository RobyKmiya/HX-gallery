//! D2 · 受控 WebView 下载器（M1 最小闭环）
//!
//! 只做 exhentai 作品页的「打开 → 解析首页 → 打包 CBZ → 入库」。
//! 页面侧只解析与取字节，Rust 侧负责落盘与入库。
//!
//! ## 为什么是「后台线程 + 事件回报」而不是一个长命令
//!
//! 管理员 2026-11-19 反馈「粘贴链接后点击直接卡死」。实测：点击后界面变成
//! 「抓取中…」，但浏览窗口根本没建出来，命令一直挂到 60 秒超时才结束，
//! 期间无法取消、没有任何反馈——用户感知就是卡死。
//!
//! 所以这里把整个流程拆到后台线程，命令本身**立即返回**：
//! `d2_start` → 立刻 Ok，前端进入等待态并可随时 `d2_cancel`；
//! 后台线程负责建窗口、轮询解析、打包入库，结束时 `emit("d2://done")`。
//! 建窗口这一步再套 30 秒超时，避免 WebView2 初始化卡住时无人收场。

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
use tauri::utils::config::WebviewUrl;
use tauri::webview::{PageLoadEvent, WebviewWindow};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindowBuilder};
use zip::write::SimpleFileOptions;

use crate::models::{ImportOptions, ImportReport, TagDraft};
use crate::{importer, paths, AppState};

/// 建窗口最多等这么久；超时说明 WebView2 初始化卡住，直接报错收场。
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
/// 整个抓取流程的上限，超过就报超时，不再无限等。
const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
/// 页面打开后多久仍没有任何解析结果，就提示用户检查登录/代理。
const NO_RESULT_HINT: Duration = Duration::from_secs(30);

static PAGE_READY: AtomicBool = AtomicBool::new(false);
static PARSE_STARTED: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);
static WINDOW: Mutex<Option<WebviewWindow>> = Mutex::new(None);

/// 注入到 exhentai 页面里的解析脚本。
///
/// 结果写在 `window.__d2Result`，Rust 侧轮询读取；这样避开 WebView2
/// `eval_with_callback` 不等待 Promise 的限制。
const D2_INIT_SCRIPT: &str = r###"
(function () {
  window.__d2Result = null;
  function clean(value) { return (value || "").trim(); }
  function absolute(href) {
    try { return new URL(href, location.href).href; } catch (e) { return href; }
  }
  function text(selector) {
    var el = document.querySelector(selector);
    return el ? clean(el.textContent) : "";
  }
  function firstHref(selectors) {
    for (var i = 0; i < selectors.length; i++) {
      var el = document.querySelector(selectors[i]);
      if (el && el.href) return absolute(el.href);
    }
    return "";
  }
  window.__d2Parse = async function () {
    try {
      var titles = document.querySelectorAll("#gd2 h1");
      var title = titles.length ? clean(titles[0].textContent) : "";
      var originTitle = titles.length > 1 ? clean(titles[1].textContent) : "";
      if (!title) title = originTitle || "UNTITLE";

      var tags = {};
      var category = text("#gdc > div");
      if (category) tags["category"] = [category];
      var uploader = text("#gdn > a");
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

      var pageUrl = firstHref(["#gdt .gdtl a", "#gdt .gdtm > div a", "#gdt > a"]);
      if (!pageUrl) {
        throw new Error("未找到第一张图片链接，请确认已登录且打开的是作品详情页");
      }

      var html = await fetch(pageUrl).then(function (r) { return r.text(); });
      var doc = new DOMParser().parseFromString(html, "text/html");
      var original =
        (doc.querySelector("#image-resize-notice > a") || {}).href ||
        (doc.querySelector("#original > a") || {}).href ||
        "";
      var img = doc.querySelector("#img");
      var normal =
        (img && (img.getAttribute("data-src") || img.src)) ||
        "";
      var imageUrl = original || normal;
      if (!imageUrl) throw new Error("图片页里没找到原图地址");

      var response = await fetch(absolute(imageUrl));
      if (!response.ok) throw new Error("图片下载失败，HTTP " + response.status);
      var blob = await response.blob();
      var base64 = await new Promise(function (resolve, reject) {
        var reader = new FileReader();
        reader.onload = function () {
          resolve(String(reader.result).split(",")[1] || "");
        };
        reader.onerror = reject;
        reader.readAsDataURL(blob);
      });

      window.__d2Result = {
        ok: true,
        title: title,
        tags: tags,
        pageUrl: pageUrl,
        imageUrl: absolute(imageUrl),
        imageBase64: base64,
        mime: blob.type || "image/jpeg"
      };
    } catch (error) {
      window.__d2Result = {
        ok: false,
        error: String((error && error.message) || error)
      };
    }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PagePayload {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: HashMap<String, Vec<String>>,
    #[serde(default)]
    page_url: String,
    #[serde(default)]
    image_url: String,
    #[serde(default)]
    image_base64: String,
    #[serde(default)]
    mime: String,
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

fn sanitize_file_name(input: &str) -> String {
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
/// 注意：`eval_with_callback` 必须由**非主线程**发起在这里没有硬性要求，
/// 但我们要保证主线程不被脚本执行阻塞，所以调用方都是后台线程。
fn eval_json(window: &WebviewWindow, expression: &str, wait: Duration) -> Result<String, String> {
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

fn build_window(app: &AppHandle, url: tauri::Url, label: String) -> Result<WebviewWindow, String> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("画匣 · D2 浏览")
        .inner_size(1180.0, 860.0)
        .initialization_script(D2_INIT_SCRIPT)
        .on_page_load(|_window, payload| {
            // 这个回调跑在事件循环（主线程）上，只做标记，绝不在这里执行脚本，
            // 否则主线程被页面拖住，整个应用都会失去响应。
            if !matches!(payload.event(), PageLoadEvent::Finished) {
                return;
            }
            let href = payload.url().as_str();
            if href.contains("/g/") || href.contains("/gallery/") {
                PAGE_READY.store(true, Ordering::SeqCst);
            }
        })
        .build()
        .map_err(|error| format!("打开浏览窗口失败：{error}"))
}

fn close_window() {
    if let Ok(mut guard) = WINDOW.lock() {
        if let Some(window) = guard.take() {
            let _ = window.close();
        }
    }
}

fn fetch_flow(
    app: &AppHandle,
    url: String,
    root: PathBuf,
    library_id: String,
    parsed: tauri::Url,
) -> Result<D2FetchReport, String> {
    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    status(app, "正在打开浏览窗口…");

    // 建窗口放到独立线程并加超时：WebView2 初始化卡住时也不至于永远没有下文。
    let (tx, rx) = mpsc::channel::<Result<WebviewWindow, String>>();
    let app_for_window = app.clone();
    std::thread::spawn(move || {
        let result = build_window(&app_for_window, parsed, label);
        let _ = tx.send(result);
    });
    let window = match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(Ok(window)) => window,
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err("打开浏览窗口超时（30 秒），WebView2 没有响应".to_string()),
    };
    log(app, "窗口已创建");
    if let Ok(mut guard) = WINDOW.lock() {
        *guard = Some(window.clone());
    }
    status(app, "窗口已打开：请登录，并进入作品详情页（地址含 /g/）");

    let started = Instant::now();
    let mut last_error = String::new();
    let mut parsed_payload: Option<PagePayload> = None;
    let mut parse_kicked = false;
    let mut blank_checked = false;

    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            close_window();
            return Err("已取消".to_string());
        }
        if started.elapsed() > FETCH_TIMEOUT {
            close_window();
            let hint = if !PAGE_READY.load(Ordering::SeqCst) {
                "目标页面在 3 分钟内没有打开。请确认：① 系统代理已开启（WebView 走系统代理）；② 已在该窗口里登录 exhentai；③ 打开的是作品详情页（地址含 /g/）。"
            } else {
                "页面已打开，但 3 分钟内没有解析出结果。请确认进入的是作品详情页，且图片页能正常显示。"
            };
            return Err(format!("抓取超时：{hint}"));
        }

        // 页面进入作品页后才触发一次解析。
        if PAGE_READY.load(Ordering::SeqCst) && !parse_kicked {
            parse_kicked = true;
            let _ = window.eval("window.__d2Parse && window.__d2Parse()");
            status(app, "已进入作品页，正在解析并下载第一页…");
        }
        if PAGE_READY.load(Ordering::SeqCst) {
            PARSE_STARTED.store(true, Ordering::SeqCst);
        }

        // 空白页诊断：exhentai 未登录会 302 到 `?poni=no` 然后显示纯白，
        // 用户看到的就是「连登录界面都打不开」。与其干等 3 分钟，不如当场说清楚。
        if !PAGE_READY.load(Ordering::SeqCst)
            && !blank_checked
            && started.elapsed() > Duration::from_secs(10)
        {
            blank_checked = true;
            if let Ok(info) = eval_json(
                &window,
                "JSON.stringify({h:location.href,n:(document.body?document.body.innerText.trim().length:0)})",
                Duration::from_secs(3),
            ) {
                log(app, &format!("页面状态：{info}"));
                if info.contains("poni=no") || info.contains("\"n\":0") {
                    status(
                        app,
                        "当前页面是空白。exhentai 未登录一定白屏：请先用上方「登录 e-hentai」，\
                         登录成功后再回到 exhentai。",
                    );
                }
            }
        }

        let json = match eval_json(
            &window,
            "window.__d2Result ? JSON.stringify(window.__d2Result) : ''",
            Duration::from_secs(3),
        ) {
            Ok(value) => value,
            Err(error) => {
                log(app, &format!("读取页面结果失败：{error}"));
                String::new()
            }
        };

        if !json.trim().is_empty() {
            let payload: PagePayload = serde_json::from_str(&json)
                .map_err(|error| format!("解析页面结果失败：{error}"))?;
            if payload.ok {
                parsed_payload = Some(payload);
                break;
            }
            last_error = payload.error.unwrap_or_else(|| "页面解析失败".to_string());
            break;
        }

        let wait_ms = if started.elapsed() > NO_RESULT_HINT {
            800
        } else {
            500
        };
        std::thread::sleep(Duration::from_millis(wait_ms));
    }

    close_window();

    let payload = match parsed_payload {
        Some(payload) => payload,
        None => return Err(if last_error.is_empty() {
            "没有取到页面结果".to_string()
        } else {
            last_error
        }),
    };

    status(app, "正在打包 CBZ…");
    let bytes = STANDARD
        .decode(payload.image_base64.as_bytes())
        .map_err(|error| format!("图片数据解码失败：{error}"))?;
    if bytes.is_empty() {
        return Err("图片数据为空".to_string());
    }

    let staging = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("获取缓存目录失败：{error}"))?
        .join("d2")
        .join("staging");
    fs::create_dir_all(&staging).map_err(|error| format!("创建暂存目录失败：{error}"))?;

    let cbz_path: PathBuf = staging.join(format!(
        "{}-{}.cbz",
        sanitize_file_name(&payload.title),
        uuid::Uuid::new_v4()
    ));
    let file = fs::File::create(&cbz_path).map_err(|error| format!("创建 CBZ 失败：{error}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let file_name = format!("0001.{}", extension_for_mime(&payload.mime));
    writer
        .start_file(file_name, options)
        .map_err(|error| format!("写入 CBZ 失败：{error}"))?;
    writer
        .write_all(&bytes)
        .map_err(|error| format!("写入 CBZ 失败：{error}"))?;
    writer
        .finish()
        .map_err(|error| format!("写入 CBZ 失败：{error}"))?;

    status(app, "正在入库…");
    let covers_dir = paths::covers_dir(app, &library_id)?;
    let _guard = crate::try_maintenance()?;
    let options = ImportOptions {
        add_to_collection: None,
        preserve_structure: None,
        tags: tag_drafts(&payload.tags),
    };
    let import_report = importer::import_blocking(
        app,
        &root,
        &covers_dir,
        vec![cbz_path.clone()],
        options,
    );

    Ok(D2FetchReport {
        title: payload.title,
        source_url: url,
        page_url: payload.page_url,
        image_url: payload.image_url,
        cbz_path: cbz_path.to_string_lossy().to_string(),
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

    if PARSE_STARTED.swap(false, Ordering::SeqCst) {
        // 上一轮已结束，清个标记继续。
    }
    CANCELLED.store(false, Ordering::SeqCst);
    PAGE_READY.store(false, Ordering::SeqCst);

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

/// 在已开的浏览窗口上执行一小段脚本。
fn with_window<F>(f: F) -> Result<(), String>
where
    F: FnOnce(&WebviewWindow) -> Result<(), String>,
{
    let guard = WINDOW.lock().map_err(|_| "浏览窗口状态异常".to_string())?;
    match guard.as_ref() {
        Some(window) => f(window),
        None => Err("浏览窗口还没打开".to_string()),
    }
}

/// 打开 / 导航到一个网址用于**浏览**（不是抓取）。
///
/// 不设域名白名单：站点地址千变万化，硬编码只会把用户挡在登录页外面
/// （2026-11-20 管理员反馈「连登录界面都打不开，白名单太笨重」）。
/// 是否可抓取由解析器决定，与能否浏览解耦。
#[tauri::command]
pub fn d2_open(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = normalize_url(&url)?;

    // 窗口还在：直接导航，不重建（保留登录态与历史）
    if let Ok(mut guard) = WINDOW.lock() {
        if let Some(window) = guard.as_ref() {
            match window.navigate(parsed.clone()) {
                Ok(()) => {
                    log(&app, &format!("导航到 {parsed}"));
                    return Ok(());
                }
                Err(_) => *guard = None, // 窗口已被用户关掉，下面重建
            }
        }
    }

    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    let app_for_window = app.clone();
    let target = parsed.to_string();
    // 建窗会阻塞，必须离开调用线程，命令本身立刻返回。
    std::thread::spawn(move || {
        status(&app_for_window, "正在打开浏览窗口…");
        match build_window(&app_for_window, parsed, label) {
            Ok(window) => {
                if let Ok(mut guard) = WINDOW.lock() {
                    *guard = Some(window);
                }
                log(&app_for_window, &format!("窗口已创建：{target}"));
                status(&app_for_window, "窗口已打开，可以直接浏览。进入作品页后回来点「开始抓取」。");
            }
            Err(error) => {
                log(&app_for_window, &format!("建窗失败：{error}"));
                let _ = app_for_window.emit("d2://error", error);
            }
        }
    });
    Ok(())
}

/// 后退。
#[tauri::command]
pub fn d2_back() -> Result<(), String> {
    with_window(|window| {
        window
            .eval("history.back()")
            .map_err(|error| format!("后退失败：{error}"))
    })
}

/// 前进。
#[tauri::command]
pub fn d2_forward() -> Result<(), String> {
    with_window(|window| {
        window
            .eval("history.forward()")
            .map_err(|error| format!("前进失败：{error}"))
    })
}

/// 刷新。
#[tauri::command]
pub fn d2_reload() -> Result<(), String> {
    with_window(|window| {
        window
            .eval("location.reload()")
            .map_err(|error| format!("刷新失败：{error}"))
    })
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
