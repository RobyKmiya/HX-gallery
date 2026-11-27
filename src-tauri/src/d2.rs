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

/// 宿主（D2 面板「浏览器设置」）填的主页。
///
/// 以前这里写死 e-hentai，面板里改主页对工具栏 ⌂ 一点作用都没有；现在由
/// `d2_open` 每次带上来，浏览器首次建窗时用它（settings.json 存在则以后者为准）。
static HOST_HOME: Mutex<Option<String>> = Mutex::new(None);

/// 注入到站点页面里的解析脚本。只做解析与取字节，结果写进 `window.__d2*`，
/// Rust 侧轮询读取（避开 WebView2 `eval_with_callback` 不等待 Promise 的限制）。
const D2_INIT_SCRIPT: &str = r###"
(function () {
  function clean(value) { return (value || "").trim(); }
  function abs(href, base) { try { return new URL(href, base || location.href).href; } catch (e) { return href; } }
  function fetchText(url) {
    return fetch(url, { credentials: "include" }).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status);
      return r.text();
    });
  }
  function fetchJSON(url) { return fetchText(url).then(function (t) { return JSON.parse(t); }); }
  function addTag(tags, key, value) {
    if (!key || !value) return;
    (tags[key] = tags[key] || []).push(value);
  }
  function tagsFromLinks(selector, ns) {
    var tags = {};
    var links = document.querySelectorAll(selector);
    for (var i = 0; i < links.length; i++) addTag(tags, ns, clean(links[i].textContent));
    return tags;
  }
  function toBase64(blob) {
    return new Promise(function (resolve, reject) {
      var reader = new FileReader();
      reader.onload = function () { resolve(String(reader.result).split(",")[1] || ""); };
      reader.onerror = reject;
      reader.readAsDataURL(blob);
    });
  }
  async function directGrab(imageUrl) {
    var response = await fetch(imageUrl, { credentials: "include" });
    if (!response.ok) throw new Error("图片下载失败，HTTP " + response.status);
    var blob = await response.blob();
    return { imageUrl: imageUrl, mime: blob.type || "image/jpeg", base64: await toBase64(blob) };
  }

  // ---- 站点分派：按 host 识别解析器，未识别的站点只浏览不抓取 ----
  function siteOf() {
    var h = (location.hostname || "").toLowerCase().replace(/^www\./, "");
    if (h === "exhentai.org" || h === "e-hentai.org") return "eh";
    if (h.indexOf("nhentai") !== -1) return "nh";
    if (h.indexOf("18comic") !== -1 || h.indexOf("jmcomic") !== -1 || h.indexOf("comic18") !== -1) return "jm";
    if (h.indexOf("wnacg") !== -1 || /^wn\d{2}\./.test(h)) return "wn";
    return "";
  }

  // ---- exhentai / e-hentai（原逻辑，保持不变） ----
  function ehMeta() {
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
      var links = cells[1].querySelectorAll("a");
      for (var a = 0; a < links.length; a++) addTag(tags, key, clean(links[a].textContent));
    }
    return { title: title, tags: tags };
  }
  async function ehCollect() {
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
      try { html = await fetchText(nextHref); } catch (e) { break; }
      doc = new DOMParser().parseFromString(html, "text/html");
    }
    return urls;
  }
  async function ehGrabOne(pageUrl) {
    var doc = new DOMParser().parseFromString(await fetchText(pageUrl), "text/html");
    var original = (doc.querySelector("#image-resize-notice > a") || {}).href ||
                   (doc.querySelector("#original > a") || {}).href || "";
    var img = doc.querySelector("#img");
    var normal = (img && (img.getAttribute("data-src") || img.src)) || "";
    var imageUrl = original || normal;
    if (!imageUrl) throw new Error("图片页里没找到原图地址");
    return directGrab(abs(imageUrl, pageUrl));
  }
  function ehReady() { return !!document.querySelector("#gdt"); }

  // ---- nhentai：走官方 API /api/v2/galleries/{id}，页表 + tag 一次拿全 ----
  function nhId() { var m = location.pathname.match(/\/g\/(\d+)/); return m ? m[1] : ""; }
  function nhMeta() {
    var t = clean((document.querySelector("#info h1") || {}).textContent) ||
            clean((document.querySelector(".info h1") || {}).textContent) ||
            ("nhentai-" + (nhId() || ""));
    return { title: t, tags: tagsFromLinks("#tags a.tag .name, #tags a.tag, .tag_name", "tag") };
  }
  async function nhCollect() {
    var id = nhId();
    if (!id) throw new Error("nhentai 作品页地址应含 /g/数字/");
    var data = await fetchJSON("/api/v2/galleries/" + id);
    var mediaId = data.media_id || (data.images && data.images.media_id);
    var pages = (data.images && data.images.pages) || data.pages || [];
    var extOf = { j: "jpg", p: "png", g: "gif", w: "webp" };
    var out = [];
    for (var i = 0; i < pages.length; i++) {
      var p = pages[i];
      var ext = extOf[p.t] || "jpg";
      var tail = p.path || ((p.number || i + 1) + "." + ext);
      out.push("direct|" + abs("https://i.nhentai.net/galleries/" + mediaId + "/" + tail));
    }
    if (!out.length) throw new Error("nhentai API 没返回图片页");
    // API 异步拿到的数据先缓存，Rust 侧随后读 __d2Meta 时直接返回。
    if (data.title) {
      window.__d2MetaCache = {
        title: clean(data.title.english || data.title.japanese || "") || ("nhentai-" + id),
        tags: tagsFromLinks("#tags a.tag .name, #tags a.tag, .tag_name", "tag")
      };
      var apiTags = {};
      var rawTags = data.tags || [];
      for (var k = 0; k < rawTags.length; k++) addTag(apiTags, rawTags[k].type, clean(rawTags[k].name));
      Object.keys(apiTags).forEach(function (key) {
        window.__d2MetaCache.tags[key] = (window.__d2MetaCache.tags[key] || []).concat(apiTags[key]);
      });
    }
    return out;
  }
  function nhReady() { return !!nhId(); }

  // ---- 18comic（禁漫）：章节入口 → 阅读页 .owl-carousel-page 取 data-src ----
  function jmMeta() {
    var t = clean((document.querySelector(".panel-heading h2") || {}).textContent) || "18comic";
    var tags = {};
    var spans = document.querySelectorAll("div.tag-block > span");
    for (var i = 0; i < spans.length; i++) {
      addTag(tags, spans[i].getAttribute("data-type") || "tag", clean(spans[i].textContent));
    }
    return { title: t, tags: tags };
  }
  function jmReadUrl(href) {
    var u = abs(href);
    return u + (u.indexOf("?") === -1 ? "?" : "&") + "read_mode=read-by-page";
  }
  async function jmCollect() {
    var readUrls = [];
    var eps = document.querySelectorAll(".visible-lg .episode > ul > a");
    for (var i = 0; i < eps.length; i++) readUrls.push(jmReadUrl(eps[i].href));
    if (!readUrls.length) {
      var block = document.querySelector(".visible-lg .read-block");
      var a = block && (block.querySelector("a") ||
              (block.firstElementChild && block.firstElementChild.querySelector("a")));
      if (a) readUrls.push(jmReadUrl(a.href));
    }
    if (!readUrls.length) throw new Error("18comic 作品页没找到章节入口");
    var seen = {};
    var out = [];
    for (var r = 0; r < readUrls.length; r++) {
      var doc = new DOMParser().parseFromString(await fetchText(readUrls[r]), "text/html");
      var imgs = doc.querySelectorAll(".owl-carousel-page > .center > img");
      for (var j = 0; j < imgs.length; j++) {
        var src = imgs[j].getAttribute("data-src") || imgs[j].getAttribute("src");
        if (src && !seen[src]) { seen[src] = 1; out.push("direct|" + abs(src, readUrls[r])); }
      }
    }
    if (!out.length) throw new Error("18comic 章节里没解析出图片（可能需要登录或站点改版）");
    window.__d2MetaCache = jmMeta();
    return out;
  }
  function jmReady() { return !!document.querySelector(".panel-heading h2, .episode"); }

  // ---- wnacg（绅士漫画）：图库页 var imglist 一次拿全 ----
  function wnMeta() {
    var t = clean((document.querySelector("#bodywrap > h2") || {}).textContent) || "wnacg";
    return { title: t, tags: tagsFromLinks(".asTB .tagshow", "tag") };
  }
  async function wnCollect() {
    var m = location.href.match(/aid-(\d+)/);
    if (!m) throw new Error("wnacg 作品页地址应含 aid-数字");
    var aid = m[1];
    var galleryUrl = abs("/photos-gallery-aid-" + aid + ".html");
    var html = await fetchText(galleryUrl);
    var block = html.match(/var\s+imglist\s*=\s*(\[[\s\S]*?\])\s*;/);
    var out = [];
    if (block) {
      var re = /url:\s*["'](.*?)["']/g;
      var u;
      while ((u = re.exec(block[1])) !== null) out.push("direct|" + abs(u[1], galleryUrl));
    }
    if (!out.length) throw new Error("wnacg 图库页没解析出 imglist（可能需要登录或站点改版）");
    window.__d2MetaCache = wnMeta();
    return out;
  }
  function wnReady() { return /aid-\d+/.test(location.href); }

  // ---- 统一入口：Rust 侧只认这三个函数和 __d2Probe ----
  window.__d2Probe = function () {
    var site = siteOf();
    var ready = site === "eh" ? ehReady() : site === "nh" ? nhReady()
              : site === "jm" ? jmReady() : site === "wn" ? wnReady() : false;
    return { site: site, ready: !!ready };
  };
  window.__d2Meta = function () {
    if (window.__d2MetaCache) return window.__d2MetaCache;
    var site = siteOf();
    if (site === "nh") return nhMeta();
    if (site === "jm") return jmMeta();
    if (site === "wn") return wnMeta();
    return ehMeta();
  };
  window.__d2Collect = async function () {
    var site = siteOf();
    if (site === "eh") return ehCollect();
    if (site === "nh") return nhCollect();
    if (site === "jm") return jmCollect();
    if (site === "wn") return wnCollect();
    throw new Error("这个站点还没有解析器：" + location.hostname);
  };
  window.__d2GrabOne = async function (pageUrl) {
    // 新站点收集阶段已给出图片直链，用 direct| 前缀区分（exhentai 仍是图片页 URL）。
    if (pageUrl.indexOf("direct|") === 0) return directGrab(pageUrl.slice(7));
    return ehGrabOne(pageUrl);
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
    adblock: bool,
    home: &str,
) -> Result<String, String> {
    let app_for_log = app.clone();
    let home_url = tauri::Url::parse(home)
        .unwrap_or_else(|_| tauri::Url::parse("https://e-hentai.org/").expect("常量 URL 合法"));
    // 窗口销毁/状态清理都在匣浏览器引擎内部完成（browser.rs）。
    let opened = crate::browser::open(
        app,
        url,
        label,
        crate::browser::BrowserOptions {
            title: "画匣 · 内建浏览器".to_string(),
            home: home_url,
            extra_init_scripts: vec![D2_INIT_SCRIPT.to_string()],
            userscript_store: crate::userscripts::store_dir(app),
            size: (1180.0, 860.0),
            initial_rect: None,
            adblock,
            picker: true,
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

/// 解析出「现在真正在跑的那个浏览窗口」。
///
/// 记录的 label 只由 `d2_open` 写入，其它入口（例如直接从导航开的窗口、
/// 或窗口被关掉后重开）会让这条记录变空/过期；过期时原来的实现会一路
/// 报「浏览器不存在」。这里先验活，再退回扫描存活窗口，并顺手修正记录。
pub fn resolve_label() -> Option<String> {
    if let Some(label) = current_label() {
        if crate::browser::is_alive(&label) {
            return Some(label);
        }
    }
    let found = crate::browser::first_alive_label()?;
    if let Ok(mut guard) = BROWSER_LABEL.lock() {
        *guard = Some(found.clone());
    }
    Some(found)
}

fn close_window() {
    if let Some(label) = current_label() {
        let _ = crate::browser::close_browser(&label);
    }
}

/// 等页面进入受支持的作品页。判定交给注入脚本的 `__d2Probe()`（按站点分派），
/// 不再写死 exhentai 的 `#gdt`；未识别站点直接给出明确提示。
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
                 ② 已在该窗口里登录站点；③ 打开的是受支持站点的作品详情页。"
            } else {
                "作品页打开了，但没解析出图片列表。请确认进入的是作品详情页。"
            };
            return Err(format!("抓取超时：{hint}"));
        }

        if let Ok(state) = eval_json(
            window,
            "JSON.stringify({p: (window.__d2Probe ? window.__d2Probe() : {site:'',ready:false}), \
             h: location.href, n: (document.body ? document.body.innerText.trim().length : 0)})",
            Duration::from_secs(3),
        ) {
            if state.contains("\"ready\":true") {
                return Ok(());
            }
            if state.contains("\"site\":\"\"") {
                return Err(format!(
                    "当前站点还没有抓取解析器（{state}）。可浏览任意站点；抓取支持 exhentai / e-hentai / nhentai / 18comic / wnacg。"
                ));
            }
            if !blank_checked && started.elapsed() > Duration::from_secs(10) {
                blank_checked = true;
                if state.contains("poni=no") || state.contains("\"n\":0") {
                    status(
                        app,
                        "当前页面是空白。站点未登录可能白屏：请先用工具栏地址栏登录，登录成功后再回到作品页。",
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

/// 下载面板用的一次扫描结果。
pub struct ActiveGallery {
    pub label: String,
    pub url: String,
    pub title: String,
    pub site: String,
    pub entries: Vec<String>,
}

/// 扫描当前浏览窗口的激活标签，拿到整部作品的元信息与全部图片条目。
///
/// 条目两种形态都认：exhentai 给的是图片页 URL，其余站点给 `direct|` 直链。
pub fn scan_active(app: &AppHandle) -> Result<ActiveGallery, String> {
    let label = resolve_label()
        .ok_or("画匣浏览器还没打开：先点左侧「画匣浏览器」进到作品页再下载")?;
    let window = crate::browser::active_webview_of(&label)?;
    let url = window
        .url()
        .map_err(|error| format!("拿不到当前页地址：{error}"))?
        .to_string();

    // 扫描是同步等人点按钮的，不能像后台抓取那样等 45 分钟：
    // 90 秒内没认出作品页就直接报错，用户还能去别的页面重试。
    let deadline = Instant::now() + Duration::from_secs(90);
    status(app, "正在解析作品页…");
    wait_for_gallery(&window, app, deadline)?;

    let site = eval_json(
        &window,
        "JSON.stringify(window.__d2Probe ? window.__d2Probe() : {})",
        Duration::from_secs(3),
    )
    .ok()
    .filter(|raw| !raw.trim().is_empty())
    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    .and_then(|value| {
        value
            .get("site")
            .and_then(|site| site.as_str())
            .map(|site| site.to_string())
    })
    .unwrap_or_default();

    let meta = meta_of(&window);
    status(app, "正在收集全部图片页…");
    let entries = collect_page_urls(&window, deadline)?;
    if entries.is_empty() {
        return Err("没有解析到任何图片页：确认进的是作品详情页，且已在该站点登录".to_string());
    }

    let title = if meta.title.trim().is_empty() {
        "UNTITLE".to_string()
    } else {
        meta.title
    };
    Ok(ActiveGallery {
        label,
        url,
        title,
        site,
        entries,
    })
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
    // 优先挑「已经打开目标站点」的浏览器：管理员在内置浏览器（工作台标签）里登录过
    // nhentai，但 BROWSER_LABEL 只有独立窗口那条路径会写，登录态被漏掉，抓取就退到
    // 另一台没登录的窗口上（2026-11-26 反馈：明明登录了还报未登录）。
    let target_host = parsed.host_str().unwrap_or("").to_string();
    let by_site = crate::browser::alive_labels().into_iter().find(|label| {
        crate::browser::active_tab_url(label)
            .map(|url| {
                let stripped = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://");
                !target_host.is_empty() && stripped.contains(&target_host)
            })
            .unwrap_or(false)
    });
    let label = by_site
        .or_else(|| current_label().filter(|label| crate::browser::is_alive(label)))
        .or_else(resolve_label)
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
            "没有解析到任何图片页。请确认打开的是受支持站点的作品详情页且已登录。".to_string(),
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
    let _guard = crate::wait_maintenance(std::time::Duration::from_secs(180))?;
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
        // 新站点的页 URL 带 internal `direct|` 前缀，回传前端前剥掉。
        page_url: page_urls
            .first()
            .map(|value| value.strip_prefix("direct|").unwrap_or(value).to_string())
            .unwrap_or_default(),
        image_url: first_image,
        cbz_path: cbz_path.to_string_lossy().to_string(),
        pages,
        failed,
        import_report,
    })
}

/// 已写好详情页解析器的站点（抓取白名单，与浏览白名单无关）。
/// 注意注入脚本 `siteOf()` 用子串匹配（覆盖 18comic 的多个镜像域名），
/// 这里也要用同样的宽松匹配，避免面板认得、抓取却被挡。
pub fn supported_site(host: &str) -> bool {
    let owned = host.to_ascii_lowercase();
    // 注入脚本 siteOf() 会先剥掉 www.，这里保持一致（wn01/wn23 等镜像常带 www.）
    let h = owned.strip_prefix("www.").unwrap_or(&owned);
    h == "exhentai.org"
        || h == "e-hentai.org"
        || h.contains("nhentai")
        || h.contains("18comic")
        || h.contains("jmcomic")
        || h.contains("comic18")
        || h.contains("wnacg")
        || {
            // wnacg 的数字镜像域名（wn01.cc / wn99.ru 等）
            let bytes = h.as_bytes();
            bytes.len() >= 5
                && h.starts_with("wn")
                && bytes[2].is_ascii_digit()
                && bytes[3].is_ascii_digit()
                && h[4..].starts_with('.')
        }
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
    start_fetch(&app, state.inner(), url)
}

/// `d2_start` 的抓取主体。
fn start_fetch(app: &AppHandle, state: &AppState, url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "网址格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("请填写 http / https 网址".to_string());
    }
    // 这里限制的是「解析器能力」，不是浏览权限：任意站点都能用 d2_open 打开，
    // 只有已写好解析器的站点才能抓取。站点列表由 supported_site 统一维护。
    let host = parsed.host_str().unwrap_or("");
    if !supported_site(host) {
        return Err(format!(
            "抓取支持 exhentai / e-hentai / nhentai / 18comic / wnacg（{host} 的解析器还没写）。\
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

    let worker = AppHandle::clone(app);
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

/// 内嵌浏览器网页下载完成后的自动导入（网页下载 → 画匣库）。
///
/// 只处理 zip / cbz（画匣的图集容器）；其它格式明确回绝，
/// 不做「悄悄进库却打不开」的事。结果通过 `d2://download` 事件回传前端。
///
/// 不带站点元数据的入口（gallery-dl 等）走这个。
pub fn import_downloaded(app: &AppHandle, path: PathBuf) {
    let _ = import_downloaded_meta(app, path, "", &HashMap::new(), None);
}

/// 看文件头判断是不是 zip（`PK\x03\x04`，空包是 `PK\x05\x06`）。
///
/// 织机「打包下载」是 FileSaver + `<a download>`，WebView2 对 blob 下载有时
/// 只给一个随机文件名、丢扩展名。这时靠魔数认出来，别让一个真 zip 因为
/// 「看起来不是 zip」被拒之门外。
fn looks_like_zip(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 2];
    file.read_exact(&mut head).is_ok() && &head == b"PK"
}

/// 网页下载入库（带作品页元数据：标题 + 标签）。
///
/// 管理员 2026-11-24 要求：织机打包出来的就是 zip，**一键落到画匣库时顺手
/// 把 tag 也配好**，不用再进详情页一条条补。元数据由内容页的 `__d2Meta()`
/// 提供（e-hentai / nhentai / 18comic / wnacg 各有解析器）；拿不到就退化成
/// 普通导入，绝不阻断。
///
/// 返回一句能直接弹给用户看的结果文案。
pub fn import_downloaded_meta(
    app: &AppHandle,
    path: PathBuf,
    title: &str,
    tags: &HashMap<String, Vec<String>>,
    source: Option<&str>,
) -> Result<String, String> {
    // blob 下载可能丢扩展名：先看魔数，是真 zip 就按标题补回 .zip 再入库。
    let mut path = path;
    let mut ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "zip" && ext != "cbz" && looks_like_zip(&path) {
        let stem = if title.trim().is_empty() {
            "网页下载".to_string()
        } else {
            sanitize_file_name(title)
        };
        let renamed = path.with_file_name(format!("{stem}.zip"));
        match std::fs::rename(&path, &renamed) {
            Ok(()) => {
                log(
                    app,
                    &format!(
                        "下载文件没有扩展名但内容是真 zip，改名入库：{} → {}",
                        path.display(),
                        renamed.display()
                    ),
                );
                path = renamed;
                ext = "zip".to_string();
            }
            Err(error) => log(app, &format!("补 .zip 扩展名失败：{error}")),
        }
    }
    if ext != "zip" && ext != "cbz" {
        log(
            app,
            &format!("下载完成但格式不是 zip/cbz，跳过导入：{}", path.display()),
        );
        let error = if ext.is_empty() {
            "下载的文件没有扩展名，内容也不是 zip".to_string()
        } else {
            format!("只支持 zip / cbz，{ext} 请手动导入")
        };
        let _ = app.emit(
            "d2://download",
            serde_json::json!({
                "ok": false,
                "path": path.to_string_lossy(),
                "error": error
            }),
        );
        return Err(error);
    }

    let state = app.state::<AppState>();
    let prepared = {
        let Ok(guard) = state.lock() else {
            return Err("状态锁异常，稍后再试".to_string());
        };
        match guard.library.as_ref() {
            Some(library) => Ok((library.root.clone(), library.library_id.clone())),
            None => Err("还没选择库目录，下载文件先留在缓存里".to_string()),
        }
    };
    let (root, library_id) = match prepared {
        Ok(value) => value,
        Err(error) => {
            log(app, &format!("下载导入跳过：{error}"));
            let _ = app.emit(
                "d2://download",
                serde_json::json!({ "ok": false, "error": error }),
            );
            return Err(error);
        }
    };

    // 标签一次算好：作品页有解析器时是「真标签 + 一条来源标记」，
    // 没有就完全不挂标签（别给每个下载都糊一条「D2下载」）。
    let drafts = if tags.is_empty() {
        Vec::new()
    } else {
        tag_drafts(tags)
    };

    let outcome = (|| -> Result<ImportReport, String> {
        let covers = paths::covers_dir(app, &library_id)?;
        // 并行下载同时结束时在这里排队（原来 try_lock 立即失败，
        // 后完成的整本被记成「导入失败」，用户只能重新下载）。
        let _guard = crate::wait_maintenance(std::time::Duration::from_secs(180))?;
        let options = ImportOptions {
            add_to_collection: None,
            preserve_structure: None,
            // 关键：把作品页的 tag 一起挂上（管理员要的「一键导入就把 tag 配好」）。
            tags: drafts.clone(),
            path_tags: Vec::new(),
            passwords: Vec::new(),
        };
        Ok(importer::import_blocking(
            app,
            &root,
            &covers,
            vec![path.clone()],
            options,
        ))
    })();

    match outcome {
        Ok(report) => {
            let imported = report.imported;
            let batch_message = report.message.clone();
            log(
                app,
                &format!(
                    "网页下载已入库：{}（{} 本，{} 个标签）",
                    path.display(),
                    imported,
                    drafts.len()
                ),
            );
            let _ = app.emit(
                "d2://download",
                serde_json::json!({
                    "ok": true,
                    "path": path.to_string_lossy(),
                    "report": report,
                }),
            );
            if imported == 0 {
                return Err(batch_message);
            }
            // 源链接写进备注，详情页可点回原页（管理员 2026-11-26）。
            if let Some(source_url) = source.map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    if let Ok(inner) = state.lock() {
                        if let Some(library) = inner.library.as_ref() {
                            for archive_id in &report.archive_ids {
                                let existing = crate::db::get_note(&library.conn, *archive_id)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default();
                                let combined = if existing.contains(source_url) {
                                    existing
                                } else if existing.is_empty() {
                                    format!("源：{source_url}")
                                } else {
                                    format!("源：{source_url}\n{existing}")
                                };
                                let _ = crate::db::set_note(
                                    &library.conn,
                                    *archive_id,
                                    Some(&combined),
                                );
                            }
                        }
                    }
                }
            }
            // drafts 里有一条固定的「来源/D2下载」标记，提示里报给用户的是
            // 真正来自作品页的标签数，别把这条算进去。
            let site_tags = drafts.len().saturating_sub(1);
            Ok(if site_tags > 0 {
                format!(
                    "已入库：{}（已配 {} 个标签）",
                    report_title(title, &path),
                    site_tags
                )
            } else {
                format!("已入库：{}", report_title(title, &path))
            })
        }
        Err(error) => {
            log(app, &format!("网页下载入库失败：{error}"));
            let _ = app.emit(
                "d2://download",
                serde_json::json!({ "ok": false, "error": error, "path": path.to_string_lossy() }),
            );
            Err(error)
        }
    }
}

/// 入库提示里显示的名字：优先用作品页标题，没有就用文件名。
fn report_title(title: &str, path: &std::path::Path) -> String {
    let trimmed = title.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "下载文件".to_string())
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
    let label = resolve_label().ok_or("浏览窗口还没打开".to_string())?;
    let webview = crate::browser::active_webview_of(&label)?;
    f(&webview)
}

/// 打开 / 导航到一个网址用于**浏览**（不是抓取）。自带浏览器工具栏。
///
/// 不设域名白名单：站点地址千变万化，硬编码只会把用户挡在登录页外面。
/// 是否可抓取由解析器决定，与能否浏览解耦。
#[tauri::command]
pub fn d2_open(
    app: AppHandle,
    url: String,
    adblock: Option<bool>,
    home: Option<String>,
) -> Result<(), String> {
    let parsed = normalize_url(&url)?;
    crate::browser::crumb(format!("d2_open 收到 {parsed}"));
    // 主线程看门狗：GUI 卡死时能把「卡前最后做了什么」写进日志。
    crate::browser::start_watchdog(&app);
    // 宿主设置的主页（工具栏 ⌂ 用）；空值不覆盖上一次的设置。
    if let Some(home) = home.as_deref().map(str::trim) {
        if !home.is_empty() {
            if let Ok(mut guard) = HOST_HOME.lock() {
                *guard = Some(home.to_string());
            }
        }
    }

    // 浏览器还在且响应正常：激活标签直接导航（保留登录态与历史）。
    // 存活判定在引擎里用回调探针（browser::is_alive），冻住的尸体瞒不过它。
    if let Some(label) = current_label() {
        crate::browser::crumb(format!("有旧浏览器 {label}，探活"));
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
    let adblock = adblock.unwrap_or(true);
    let home = HOST_HOME
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(|| "https://e-hentai.org/".to_string());
    std::thread::spawn(move || {
        status(&app_for_window, "正在打开浏览窗口…");
        match open_browser_window(&app_for_window, parsed, label, adblock, &home) {
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

// ---------- 内置模式（P0 探针）：浏览器引擎挂进 main 窗 ----------

/// 前端在打开内置浏览器前量好的占位矩形（逻辑像素），用于第一帧摆位。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct EmbedRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 打开内置浏览器（宿主 = main 窗）。返回引擎 label，前端拿它做 embed 同步。
#[tauri::command]
pub fn d2_embed_open(
    app: AppHandle,
    url: String,
    adblock: Option<bool>,
    home: Option<String>,
    picker: Option<bool>,
    rect: Option<EmbedRect>,
) -> Result<String, String> {
    let parsed = normalize_url(&url)?;
    crate::browser::start_watchdog(&app);
    if let Some(home) = home.as_deref().map(str::trim) {
        if !home.is_empty() {
            if let Ok(mut guard) = HOST_HOME.lock() {
                *guard = Some(home.to_string());
            }
        }
    }
    let home = HOST_HOME
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(|| "https://e-hentai.org/".to_string());
    let host = app
        .get_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    let label = format!("d2-embed-{}", uuid::Uuid::new_v4().simple());
    let app_for_thread = app.clone();
    let label_for_thread = label.clone();
    let adblock = adblock.unwrap_or(true);
    let picker = picker.unwrap_or(true);
    let initial_rect = rect.map(|r| (r.x, r.y, r.w, r.h));
    // add_child 不能在同步命令回调里跑（会死锁），照旧线程 + 日志回执。
    std::thread::spawn(move || {
        match open_browser_window_embedded(
            &app_for_thread,
            parsed,
            label_for_thread,
            adblock,
            &home,
            picker,
            host,
            initial_rect,
        ) {
            Ok(opened) => log(&app_for_thread, &format!("内置浏览器已挂载：{opened}")),
            Err(error) => {
                log(&app_for_thread, &format!("内置挂载失败：{error}"));
                let _ = app_for_thread.emit("d2://error", error);
            }
        }
    });
    Ok(label)
}

/// 内置模式摆位/显隐/拆除（label 由 d2_embed_open 返回）。
#[tauri::command]
pub fn d2_embed_sync(label: String, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    crate::browser::embed_sync(&label, x, y, w, h)
}

#[tauri::command]
pub fn d2_embed_show(label: String, visible: bool) -> Result<(), String> {
    crate::browser::embed_show(&label, visible)
}

#[tauri::command]
pub fn d2_embed_close(label: String) -> Result<(), String> {
    crate::browser::embed_close(&label)
}

/// 读浏览器设置（settings.json）。主设置对话框 / D2 面板用它展示同一份配置
///（1.1.96 整合：浏览器设置的唯一真源，不再各自存 localStorage）。
#[tauri::command]
pub fn browser_settings_get() -> Result<serde_json::Value, String> {
    serde_json::to_value(crate::browser::load_settings_with_fallback(None))
        .map_err(|e| format!("序列化设置失败: {e}"))
}

/// 写浏览器设置（settings.json）。只改传上来的字段：先读现值再合并，
/// 避免壳侧某个开关把主页/缩放等其它项冲掉。
#[tauri::command]
pub fn browser_settings_set(patch: serde_json::Value) -> Result<(), String> {
    let mut current = crate::browser::load_settings_with_fallback(None);
    let merged = merge_json(serde_json::to_value(&current).map_err(|e| e.to_string())?, patch);
    current = serde_json::from_value(merged).map_err(|e| format!("设置格式不对: {e}"))?;
    current = current.sanitized();
    crate::browser::save_settings(&current)?;
    // 立即作用到运行中的浏览器（开着的话）：广告拦截全局开关 + 圆点显隐。
    crate::browser::set_adblock_enabled(current.adblock);
    crate::browser::apply_picker_to_tabs(current.picker);
    Ok(())
}

/// 浅合并：patch 里的键覆盖 base，其余保留（只处理一层对象，够用）。
fn merge_json(mut base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
        for (key, value) in p {
            b.insert(key.clone(), value.clone());
        }
    }
    base
}

/// open_browser_window 的内置版：同一套 opts，宿主换成 main 窗。
fn open_browser_window_embedded(
    app: &AppHandle,
    url: tauri::Url,
    label: String,
    adblock: bool,
    home: &str,
    picker: bool,
    host: tauri::Window,
    initial_rect: Option<(f64, f64, f64, f64)>,
) -> Result<String, String> {
    let app_for_log = app.clone();
    let home_url = tauri::Url::parse(home)
        .unwrap_or_else(|_| tauri::Url::parse("https://e-hentai.org/").expect("常量 URL 合法"));
    crate::browser::open_embedded(
        app,
        url,
        label,
        crate::browser::BrowserOptions {
            title: "画匣 · 内建浏览器".to_string(),
            home: home_url,
            extra_init_scripts: vec![D2_INIT_SCRIPT.to_string()],
            userscript_store: crate::userscripts::store_dir(app),
            size: (900.0, 600.0),
            initial_rect,
            adblock,
            picker,
            log: Some(std::sync::Arc::new(move |message| log(&app_for_log, message))),
        },
        host,
    )
}

/// 读当前浏览窗口的地址（供「抓取当前页」一键使用，免去重新粘贴）。
#[tauri::command]
pub fn d2_current_url() -> Result<String, String> {
    let Some(label) = resolve_label() else {
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
    let Some(label) = resolve_label() else {
        return Err("浏览窗口还没打开".to_string());
    };
    crate::browser::history_action(&label, "back")
}

/// 前进。
#[tauri::command]
pub fn d2_forward() -> Result<(), String> {
    let Some(label) = resolve_label() else {
        return Err("浏览窗口还没打开".to_string());
    };
    crate::browser::history_action(&label, "forward")
}

/// 刷新。
#[tauri::command]
pub fn d2_reload() -> Result<(), String> {
    let Some(label) = resolve_label() else {
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
pub fn d2_set_cookies(
    app: tauri::AppHandle,
    cookie_header: String,
    domains: Option<Vec<String>>,
) -> Result<usize, String> {
    use tauri::Manager;

    let pairs = parse_cookie_pairs(&cookie_header);
    if pairs.is_empty() {
        return Err("没解析出任何 cookie：请粘贴 name=value 形式的内容".to_string());
    }

    // 域名列表：调用方可按站点指定（18comic/wnacg/nhentai…）；
    // 不传则默认 e-hentai + exhentai 双域。
    // 同时保留「带点」和「裸域」两个版本：WebView2 对 Domain=.xxx 与
    // Domain=xxx 的匹配行为不同，exhentai 的 sk 在裸域下才稳定生效。
    let requested = domains
        .unwrap_or_else(|| vec![".e-hentai.org".to_string(), ".exhentai.org".to_string()]);
    let mut targets: Vec<String> = Vec::new();
    for domain in requested {
        let bare = domain.trim_start_matches('.').to_ascii_lowercase();
        for candidate in [format!(".{bare}"), bare] {
            if !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
    }
    // 裸域集合：用来判断当前标签页是不是目标站点（决定要不要刷新）。
    let bare_domains: Vec<String> = targets
        .iter()
        .map(|d| d.trim_start_matches('.').to_ascii_lowercase())
        .collect();

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
            for domain in &targets {
                let mut cookie = cookie::Cookie::new(name.clone(), value.clone());
                cookie.set_domain(domain);
                cookie.set_path("/");
                // 不设 secure/httpOnly：会话 cookie 要能被站点脚本读取做校验；
                // 是否仅 HTTPS 传输由站点页面协议决定，硬设 secure 会让 HTTP 调试页失效。
                // 但过期时间必须设：不设就是会话 cookie，画匣一重启登录态全丢
                // （管理员 2026-11-26「登录状态页不保存」）。180 天足够长。
                cookie.set_max_age(cookie::time::Duration::days(180));
                if webview.set_cookie(cookie).is_ok() {
                    written += 1;
                }
            }
        }
        // 只刷新「当前页 host 命中目标域」的标签页。
        // 原来无条件刷新所有标签：若当前停在别的域（如 e-hentai），
        // 写到 .exhentai.org 的 cookie 根本不会进入该请求，看上去就像「没登上」。
        if let Ok(url) = webview.url() {
            let host = url.host_str().unwrap_or("").to_ascii_lowercase();
            let hits_target = bare_domains
                .iter()
                .any(|d| host == *d || host.ends_with(&format!(".{d}")));
            if hits_target {
                let mut refresh = url.clone();
                refresh.set_fragment(None);
                let _ = webview.navigate(refresh);
            }
        }
    }

    if written == 0 {
        return Err("写入 cookie 失败：浏览窗口可能已经关掉了".to_string());
    }

    // 顺手存一份加密快照：以后浏览窗口关掉、甚至画匣重启过，网络下载也能直接
    // 用这段 Cookie，不用再粘一次。pairs 已把「每行一个」和整段请求头都归一了。
    if let Some(host) = bare_domains.first() {
        let snapshot = pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !snapshot.is_empty() {
            let _ = crate::save_site_login_snapshot(
                &app,
                host,
                &snapshot,
                "粘贴 Cookie 时自动保存",
            );
        }
    }
    Ok(written)
}

/// 一键抓取当前浏览标签页里的全部 tag（管理员 11-23：上百个标签不能一个一个点）。
///
/// 通道设计：浏览窗口没有 IPC（安全边界），所以让注入脚本把结果写进
/// `huaxia_tags_*` 一组 cookie（每块 3000 字符，超长自动分块），Rust 侧用
/// `cookies_for_url` 读回来再组装。脚本注入是原生 eval，不受页面 CSP 限制。
#[tauri::command]
pub async fn d2_collect_tags() -> Result<String, String> {
    let label = resolve_label().ok_or_else(|| "浏览窗口还没打开".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let webview = crate::browser::active_webview(&label)?;
        let url = webview
            .url()
            .map_err(|error| format!("拿不到当前页地址: {error}"))?;

        const COLLECT_SCRIPT: &str = r#"
(function () {
  var tags = Array.prototype.slice
    .call(document.querySelectorAll('a[href*="/tag/"]'))
    .map(function (a) {
      var m = (a.getAttribute('href') || '').split('/tag/');
      if (m.length < 2) return null;
      try { return decodeURIComponent(m[1]).replace(/\+/g, ' ').trim(); }
      catch (e) { return null; }
    })
    .filter(Boolean);
  var seen = {};
  var unique = tags.filter(function (t) {
    var k = t.toLowerCase();
    if (seen[k]) return false;
    seen[k] = true;
    return true;
  });
  var enc = encodeURIComponent(unique.join('\n'));
  var chunks = Math.max(1, Math.ceil(enc.length / 3000));
  for (var i = 0; i < chunks; i++) {
    document.cookie = 'huaxia_tags_' + i + '=' + enc.slice(i * 3000, (i + 1) * 3000) + ';path=/';
  }
  document.cookie = 'huaxia_tags_count=' + chunks + ';path=/';
})();
"#;
        webview
            .eval(COLLECT_SCRIPT)
            .map_err(|error| format!("注入抓取脚本失败: {error}"))?;

        // 脚本是同步执行的，cookie 立即可读；轮询只是兜底 race。
        let mut assembled = String::new();
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let cookies = webview
                .cookies_for_url(url.clone())
                .map_err(|error| format!("读取 cookie 失败: {error}"))?;
            let count = cookies.iter().find(|c| c.name() == "huaxia_tags_count");
            let Some(count) = count else { continue };
            let chunks: usize = count.value().parse().unwrap_or(0);
            if chunks == 0 { continue; }
            let mut parts = vec![String::new(); chunks];
            for cookie in &cookies {
                if let Some(rest) = cookie.name().strip_prefix("huaxia_tags_") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        if idx < chunks {
                            parts[idx] = cookie.value().to_string();
                        }
                    }
                }
            }
            if parts.iter().any(String::is_empty) { continue; }
            assembled = parts.join("");
            break;
        }
        if assembled.is_empty() {
            return Err("没抓到标签：当前页可能不是作品详情页（本子页才有 tag 列表）".to_string());
        }

        // 清掉工作 cookie，下次抓取不会串味。
        let _ = webview.eval("for (var i = 0; i < 50; i++) { document.cookie = 'huaxia_tags_' + i + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/'; } document.cookie = 'huaxia_tags_count=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/';");

        let decoded = percent_decode(&assembled);
        let total = decoded.lines().filter(|line| !line.trim().is_empty()).count();
        if total == 0 {
            return Err("当前页没有解析出任何 tag".to_string());
        }
        Ok(decoded)
    })
    .await
    .map_err(|error| format!("抓取任务异常结束: {error}"))?
}

/// 手写的百分号解码（cookie 值由前端 encodeURIComponent 编码）。
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &raw[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
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
    fn supported_site_covers_all_parsers() {
        // 主域名（大小写不敏感）
        for host in ["exhentai.org", "E-Hentai.org", "nhentai.net", "nhentai.xxx",
                     "18comic.vip", "www.jmcomic.cc", "comic18j-codi.net", "www.wnacg.com"] {
            assert!(supported_site(host), "应支持 {host}");
        }
        // wnacg 数字镜像域名
        for host in ["wn01.cc", "wn99.ru", "www.wn23.cc"] {
            assert!(supported_site(host), "应支持镜像 {host}");
        }
        // 未写解析器的站点仍可浏览，但不能抓取
        for host in ["example.com", "pixiv.net", "danbooru.donmai.us", ""] {
            assert!(!supported_site(host), "不应支持 {host}");
        }
    }

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
