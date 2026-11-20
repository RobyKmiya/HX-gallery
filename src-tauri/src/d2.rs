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
use tauri::utils::config::WebviewUrl;
use tauri::webview::WebviewWindow;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindowBuilder};
use zip::write::SimpleFileOptions;

use crate::models::{ImportOptions, ImportReport, TagDraft};
use crate::{importer, paths, AppState};

/// 建窗口最多等这么久；超时说明 WebView2 初始化卡住，直接报错收场。
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
/// 整个抓取流程的上限。整部作品顺序下载几百张要不少时间，给足余量。
const FETCH_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// 单张图片的等待上限（从发起取字节到拿回结果）。
const GRAB_TIMEOUT: Duration = Duration::from_secs(60);
/// 页面打开后多久仍没有作品页结构，就提示用户检查登录/代理。
const NO_RESULT_HINT: Duration = Duration::from_secs(30);

static CANCELLED: AtomicBool = AtomicBool::new(false);
static WINDOW: Mutex<Option<WebviewWindow>> = Mutex::new(None);

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

/// 浏览窗口自带的「浏览器外壳」：地址栏 + 前进/后退/刷新/主页 + 缩放。
///
/// 用 Shadow DOM 挂在页面最上层：样式与站点完全隔离，既不被站点 CSS 冲掉，
/// 也不污染站点样式。纯客户端实现——浏览窗口没有 IPC 能力，站点页面拿不到
/// 画匣命令，安全边界不变。
///
/// 缩放走 CSS `zoom`（写 documentElement），工具栏自身用 `1/zoom` 抵消，
/// 效果接近 Edge 的「页面缩放 + 浏览器 UI 不缩放」。
const D2_TOOLBAR_SCRIPT: &str = r###"
(function () {
  if (window.top !== window || window.__d2Toolbar) return;
  window.__d2Toolbar = true;
  var HOST_ID = "__d2_toolbar_host";
  var zoomPct = 100;
  try {
    var saved = parseInt(sessionStorage.getItem("__d2_zoom") || "", 10);
    if (saved >= 25 && saved <= 400) zoomPct = saved;
  } catch (e) {}

  function mount() {
    if (document.getElementById(HOST_ID)) return;
    var host = document.createElement("div");
    host.id = HOST_ID;
    host.setAttribute("style", "position:fixed;inset:0;pointer-events:none;z-index:2147483647;");
    (document.documentElement || document.body).appendChild(host);
    if (!host.attachShadow) return;
    var root = host.attachShadow({ mode: "open" });

    var style = document.createElement("style");
    style.textContent = [
      ".bar{position:fixed;top:0;left:0;right:0;display:flex;align-items:center;gap:6px;",
      "padding:6px 8px;background:rgba(18,20,25,.97);border-bottom:1px solid #333842;",
      "color:#e8eaf0;font:13px 'Segoe UI','Microsoft YaHei UI',sans-serif;pointer-events:auto;",
      "box-shadow:0 2px 10px rgba(0,0,0,.35);}",
      ".bar button{height:26px;min-width:30px;border:1px solid #3a3f4a;border-radius:6px;",
      "background:#262a33;color:#e8eaf0;cursor:pointer;font-size:13px;line-height:1;}",
      ".bar button:hover{background:#313642;}",
      ".bar button.zoom{min-width:54px;font-variant-numeric:tabular-nums;}",
      ".addr{flex:1 1 auto;height:26px;min-width:80px;border:1px solid #3a3f4a;border-radius:6px;",
      "background:#0f1116;color:#e8eaf0;padding:0 10px;font:12px 'Cascadia Mono',Consolas,monospace;}",
      ".pill{position:fixed;top:6px;right:8px;height:26px;padding:0 12px;border:1px solid #3a3f4a;",
      "border-radius:13px;background:rgba(18,20,25,.92);color:#e8eaf0;cursor:pointer;",
      "font:12px 'Segoe UI','Microsoft YaHei UI',sans-serif;pointer-events:auto;}",
    ].join("");
    root.appendChild(style);

    var bar = document.createElement("div");
    bar.className = "bar";

    function mkBtn(label, title, fn) {
      var b = document.createElement("button");
      b.textContent = label;
      b.title = title;
      b.addEventListener("click", fn);
      return b;
    }

    var addr = document.createElement("input");
    addr.className = "addr";
    addr.spellcheck = false;
    addr.addEventListener("keydown", function (e) {
      if (e.key !== "Enter") return;
      var v = addr.value.trim();
      if (!v) return;
      if (!/^https?:\/\//i.test(v)) v = "https://" + v;
      location.href = v;
    });
    addr.addEventListener("focus", function () { addr.value = location.href; addr.select(); });
    function syncAddr() { if (document.activeElement !== addr) addr.value = location.href; }

    var zoomLabel = mkBtn(zoomPct + "%", "重置为 100%", function () {
      zoomPct = 100; applyZoom(); zoomLabel.textContent = "100%";
    });
    zoomLabel.className = "zoom";

    var pill = document.createElement("button");
    pill.className = "pill";
    pill.textContent = "\u2630 \u753b\u5323";
    pill.style.display = "none";

    function applyZoom() {
      var z = zoomPct / 100;
      document.documentElement.style.zoom = zoomPct === 100 ? "" : String(z);
      bar.style.zoom = zoomPct === 100 ? "" : String(1 / z);
      try { sessionStorage.setItem("__d2_zoom", String(zoomPct)); } catch (e) {}
    }
    function collapse() { bar.style.display = "none"; pill.style.display = "block"; }
    function expand() { bar.style.display = "flex"; pill.style.display = "none"; syncAddr(); }
    pill.addEventListener("click", expand);

    bar.appendChild(mkBtn("\u2190", "后退", function () { history.back(); }));
    bar.appendChild(mkBtn("\u2192", "前进", function () { history.forward(); }));
    bar.appendChild(mkBtn("\u27f3", "刷新", function () { location.reload(); }));
    bar.appendChild(mkBtn("\u2302", "回到 e-hentai", function () { location.href = "https://e-hentai.org/"; }));
    bar.appendChild(addr);
    bar.appendChild(mkBtn("\u2212", "缩小", function () {
      zoomPct = Math.max(25, zoomPct - 10); applyZoom(); zoomLabel.textContent = zoomPct + "%";
    }));
    bar.appendChild(zoomLabel);
    bar.appendChild(mkBtn("+", "放大", function () {
      zoomPct = Math.min(400, zoomPct + 10); applyZoom(); zoomLabel.textContent = zoomPct + "%";
    }));
    bar.appendChild(mkBtn("\u2303", "收起工具栏", collapse));

    root.appendChild(bar);
    root.appendChild(pill);

    applyZoom();
    syncAddr();
    setInterval(syncAddr, 1000);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", mount);
  } else {
    mount();
  }
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
/// `eval_with_callback` 不等待 Promise，所以页面侧统一用「先写全局变量、
/// 再轮询读取」的写法。调用方都在后台线程，不会阻塞主线程。
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

/// 浏览窗口是否真的活着（页面还能执行脚本并把结果送回来）。
///
/// 「卡退」的尸体有两种形态：被关掉的（句柄已死，任何调用都报错）和渲染
/// 进程冻住的（外壳还在，navigate 等调用全部「成功」但页面永远不动）。
/// 后一种光看返回值看不出来，必须发脚本探针并等回调真的回来。
fn window_is_alive(window: &WebviewWindow) -> bool {
    matches!(
        eval_json(window, "1", Duration::from_secs(5)),
        Ok(value) if value.trim() == "1"
    )
}

fn build_window(
    app: &AppHandle,
    url: tauri::Url,
    label: String,
    with_toolbar: bool,
) -> Result<WebviewWindow, String> {
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("画匣 · 内建浏览器")
        .inner_size(1180.0, 860.0)
        .initialization_script(D2_INIT_SCRIPT);
    if with_toolbar {
        builder = builder.initialization_script(D2_TOOLBAR_SCRIPT);
    }
    let window = builder
        .build()
        .map_err(|error| format!("打开浏览窗口失败：{error}"))?;
    // 用户点 × / 窗口崩掉 → 立刻清全局句柄。没有这一步，「卡退后被关掉」的
    // 窗口会在 WINDOW 里留一具尸体，d2_open / fetch_flow 的复用分支撞上它，
    // 就是「浏览器卡退之后再也打不开」。
    let label_for_close = window.label().to_string();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Ok(mut guard) = WINDOW.lock() {
                let same = guard
                    .as_ref()
                    .map(|w| w.label() == label_for_close)
                    .unwrap_or(false);
                if same {
                    *guard = None;
                }
            }
        }
    });
    Ok(window)
}

/// 建窗放到独立线程并加超时：WebView2 初始化卡住时也不至于永远没有下文。
fn open_window(
    app: &AppHandle,
    url: tauri::Url,
    label: String,
    with_toolbar: bool,
) -> Result<WebviewWindow, String> {
    let (tx, rx) = mpsc::channel::<Result<WebviewWindow, String>>();
    let app_for_window = app.clone();
    std::thread::spawn(move || {
        let result = build_window(&app_for_window, url, label, with_toolbar);
        // 调用方已经超时放弃（WebView2 初始化卡住）：迟到的窗口直接销毁，
        // 不给用户留一扇几十秒后才突然出现的幽灵窗。
        if tx.send(result.clone()).is_err() {
            if let Ok(window) = result {
                let _ = window.destroy();
            }
        }
    });
    match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err("打开浏览窗口超时（30 秒），WebView2 没有响应".to_string()),
    }
}

fn close_window() {
    if let Ok(mut guard) = WINDOW.lock() {
        if let Some(window) = guard.take() {
            let _ = window.close();
        }
    }
}

fn existing_window() -> Option<WebviewWindow> {
    WINDOW.lock().ok().and_then(|guard| guard.as_ref().cloned())
}

/// 等页面进入作品页（出现 `#gdt`）。期间对空白页做一次诊断提示。
fn wait_for_gallery(window: &WebviewWindow, app: &AppHandle, deadline: Instant) -> Result<(), String> {
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
fn collect_page_urls(window: &WebviewWindow, deadline: Instant) -> Result<Vec<String>, String> {
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
fn grab_one(window: &WebviewWindow, page_url: &str) -> Result<GrabPayload, String> {
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

fn meta_of(window: &WebviewWindow) -> MetaPayload {
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
    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    // 复用已经打开的浏览窗口：用户正在看的作品页直接抓，不再弹第二个窗口。
    let window = match existing_window() {
        Some(window) if window_is_alive(&window) => {
            let same = window
                .url()
                .map(|current| current.as_str() == parsed.as_str())
                .unwrap_or(false);
            if !same {
                let _ = window.navigate(parsed.clone());
            }
            window
        }
        maybe_stale => {
            // 尸体窗口（被关掉 / 卡退冻住）先销毁再重建，绝不复用：
            // 旧逻辑撞上冻住的窗口会在这里干等整个抓取超时。
            if let Some(stale) = maybe_stale {
                log(app, "已有浏览窗口无响应，销毁后重新打开");
                let _ = stale.destroy();
            }
            if let Ok(mut guard) = WINDOW.lock() {
                *guard = None;
            }
            status(app, "正在打开浏览窗口…");
            let window = open_window(app, parsed.clone(), label, true)?;
            if let Ok(mut guard) = WINDOW.lock() {
                *guard = Some(window.clone());
            }
            window
        }
    };
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

/// 打开 / 导航到一个网址用于**浏览**（不是抓取）。自带浏览器工具栏。
///
/// 不设域名白名单：站点地址千变万化，硬编码只会把用户挡在登录页外面。
/// 是否可抓取由解析器决定，与能否浏览解耦。
#[tauri::command]
pub fn d2_open(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = normalize_url(&url)?;

    // 窗口还在且**响应正常**：直接导航，不重建（保留登录态与历史）。
    // 「卡退」会留下两种尸体：被关掉的（调用即报错）和渲染进程冻住的
    // （调用全部「成功」但页面永远不动——2026-11-21 实测：日志连发八条
    // 「导航到」而页面纹丝不动）。存活判定必须用回调探针，不能看返回值。
    let mut stale = false;
    if let Ok(mut guard) = WINDOW.lock() {
        if let Some(window) = guard.as_ref() {
            if window_is_alive(window) {
                match window.navigate(parsed.clone()) {
                    Ok(()) => {
                        log(&app, &format!("导航到 {parsed}"));
                        return Ok(());
                    }
                    Err(_) => stale = true,
                }
            } else {
                stale = true;
            }
        }
        if stale {
            if let Some(window) = guard.take() {
                log(&app, "上次浏览窗口已无响应，先销毁再重建");
                let _ = window.destroy();
            }
        }
    }

    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    let app_for_window = app.clone();
    let target = parsed.to_string();
    // 建窗会阻塞，必须离开调用线程，命令本身立刻返回。
    std::thread::spawn(move || {
        status(&app_for_window, "正在打开浏览窗口…");
        match open_window(&app_for_window, parsed, label, true) {
            Ok(window) => {
                if let Ok(mut guard) = WINDOW.lock() {
                    *guard = Some(window);
                }
                log(&app_for_window, &format!("窗口已创建：{target}"));
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
    let guard = WINDOW.lock().map_err(|_| "浏览窗口状态异常".to_string())?;
    match guard.as_ref() {
        Some(window) => Ok(window.url().map(|url| url.to_string()).unwrap_or_default()),
        None => Ok(String::new()),
    }
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
