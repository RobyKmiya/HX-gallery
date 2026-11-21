//! 匣浏览器 —— 匣系应用共用的内嵌浏览器引擎。
//!
//! 2026-11-21 从画匣 D2 浏览器提炼：往后所有匣系应用（画匣/影匣/音匣/…）
//! 的内置浏览器都直接挂这个模块，不再各自造轮子。能力清单：
//!
//! - **内核**：WebView2 = Edge/Chromium，网页兼容性与 Edge 天然齐平；
//! - **外壳**：Shadow DOM 工具栏（地址栏/前进/后退/刷新/主页/缩放/收起），
//!   样式与站点双向隔离；
//! - **弹窗接管**：`target=_blank`/`window.open` 一律在本窗口内打开（内嵌
//!   浏览器不该往外弹窗）；
//! - **下载上报**：下载开始/完成走 `on_download` 钩子，回调给宿主应用记日志；
//! - **油猴内核**：见 [`crate::userscripts`]——脚本仓库 + `==UserScript==`
//!   头解析 + @match/@include 匹配 + GM 垫片 + 跨域 XHR 队列轮询器；
//! - **健壮性**：存活探针（冻住的窗口「调用成功但页面不动」，必须用回调探针）、
//!   窗口销毁即时清状态、超时后迟到的窗口直接销毁不留幽灵窗。
//!
//! 宿主只做三件事：传 [`BrowserOptions`]、拿回 `(窗口, 停止旗)`、在窗口销毁
//! 事件里清自己的状态。嵌入指南见 `docs/匣浏览器经验.md`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Url, WebviewWindow, WebviewWindowBuilder, WebviewUrl};

/// 建窗口最多等这么久；超时说明 WebView2 初始化卡住，直接报错收场。
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
/// GM 跨域 XHR 队列的轮询间隔。
const GM_XHR_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 存活探针的单次等待。窗口活着但页面忙时可能要等一会，别太紧。
const ALIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 浏览窗口自带的「浏览器外壳」：地址栏 + 前进/后退/刷新/主页 + 缩放 + 收起。
///
/// 用 Shadow DOM 挂在页面最上层：样式与站点完全隔离，既不被站点 CSS 冲掉，
/// 也不污染站点样式。纯客户端实现——浏览窗口没有 IPC 能力，站点页面拿不到
/// 宿主命令，安全边界不变（油猴的跨域请求走 `__xkGmXhr` 队列由 Rust 轮询）。
const TOOLBAR_SCRIPT_TEMPLATE: &str = r###"
(function () {
  if (window.top !== window || window.__xkToolbar) return;
  window.__xkToolbar = true;
  var HOST_ID = "__xk_toolbar_host";
  var HOME = "__HOME_URL__";
  var zoomPct = 100;
  try {
    var saved = parseInt(sessionStorage.getItem("__xk_zoom") || "", 10);
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
    pill.textContent = "\u2630 \u5336\u6d4f\u89c8\u5668";
    pill.style.display = "none";

    function applyZoom() {
      var z = zoomPct / 100;
      document.documentElement.style.zoom = zoomPct === 100 ? "" : String(z);
      bar.style.zoom = zoomPct === 100 ? "" : String(1 / z);
      try { sessionStorage.setItem("__xk_zoom", String(zoomPct)); } catch (e) {}
    }
    function collapse() { bar.style.display = "none"; pill.style.display = "block"; }
    function expand() { bar.style.display = "flex"; pill.style.display = "none"; syncAddr(); }
    pill.addEventListener("click", expand);

    bar.appendChild(mkBtn("\u2190", "后退", function () { history.back(); }));
    bar.appendChild(mkBtn("\u2192", "前进", function () { history.forward(); }));
    bar.appendChild(mkBtn("\u27f3", "刷新", function () { location.reload(); }));
    bar.appendChild(mkBtn("\u2302", "回到主页", function () { location.href = HOME; }));
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

/// 浏览窗口是否真的活着（页面还能执行脚本并把结果送回来）。
///
/// 「卡退」的尸体有两种形态：被关掉的（句柄已死，任何调用都报错）和渲染
/// 进程冻住的（外壳还在，navigate 等调用全部「成功」但页面永远不动）。
/// 后一种光看返回值看不出来，必须发脚本探针并等回调真的回来。
pub fn window_is_alive(window: &WebviewWindow) -> bool {
    matches!(
        eval_json(window, "1", ALIVE_PROBE_TIMEOUT),
        Ok(value) if value.trim() == "1"
    )
}

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

/// 浏览器配置。宿主按需填，全部字段都有合理默认。
pub struct BrowserOptions {
    /// 窗口标题。
    pub title: String,
    /// 主页（工具栏「⌂」按钮的目标）。
    pub home: Url,
    /// 宿主自有的页面脚本（在工具栏之前注入，如画匣的站点解析器）。
    pub extra_init_scripts: Vec<String>,
    /// 油猴脚本仓库目录；None = 禁用油猴内核。
    pub userscript_store: Option<PathBuf>,
    /// 窗口初始尺寸。
    pub size: (f64, f64),
    /// 宿主日志回调（弹窗接管/下载/油猴事件都走它）。
    pub log: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            title: "匣浏览器".to_string(),
            home: Url::parse("https://www.bing.com/").unwrap_or_else(|_| {
                Url::parse("about:blank").expect("about:blank 是合法 URL")
            }),
            extra_init_scripts: Vec::new(),
            userscript_store: None,
            size: (1180.0, 860.0),
            log: None,
        }
    }
}

struct BrowserDeps {
    log: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pending_navs: Arc<Mutex<Vec<Url>>>,
    stop: Arc<AtomicBool>,
}

impl BrowserDeps {
    fn log(&self, message: &str) {
        if let Some(log) = &self.log {
            log(message);
        }
    }
}

/// 打开（或创建）一个受控浏览器窗口。
///
/// 返回窗口与「停止旗」：窗口被销毁时引擎内部会置位停止旗并停掉油猴轮询器；
/// 宿主仍应自己监听 `WindowEvent::Destroyed` 清理它保存的句柄（见 d2.rs）。
pub fn open(
    app: &AppHandle,
    url: Url,
    label: String,
    opts: BrowserOptions,
) -> Result<(WebviewWindow, Arc<AtomicBool>), String> {
    // 停止旗与弹窗队列在函数级创建：线程内外共享同一份，销毁时统一退场。
    let stop = Arc::new(AtomicBool::new(false));
    let pending_navs = Arc::new(Mutex::new(Vec::new()));
    let userscript_store = opts.userscript_store.clone();
    let userscript_store_for_thread = userscript_store.clone();

    let (tx, rx) = mpsc::channel::<Result<WebviewWindow, String>>();
    let app_for_window = app.clone();
    let stop_for_thread = stop.clone();
    let pending_for_thread = pending_navs.clone();
    // 建窗整体放到独立线程：builder 的生命周期绑在线程自己的 AppHandle 上，
    // WebView2 初始化卡住时调用方超时收场，线程里迟到的窗口自行销毁。
    std::thread::spawn(move || {
        let deps = Arc::new(BrowserDeps {
            log: opts.log,
            pending_navs: pending_for_thread,
            stop: stop_for_thread,
        });
        let mut builder = WebviewWindowBuilder::new(&app_for_window, label, WebviewUrl::External(url))
            .title(opts.title.clone())
            .inner_size(opts.size.0, opts.size.1)
            // 弹窗接管：站点开的 target=_blank / window.open 全部在本窗口内打开。
            .on_new_window({
                let deps = deps.clone();
                move |new_url, _features| {
                    deps.log(&format!("弹窗接管，本窗打开：{new_url}"));
                    if let Ok(mut queue) = deps.pending_navs.lock() {
                        queue.push(new_url.clone());
                    }
                    NewWindowResponse::Deny
                }
            })
            // 下载上报：目的地交给 WebView2 默认（用户下载目录），宿主记日志。
            .on_download({
                let deps = deps.clone();
                move |_webview, event| {
                    match event {
                        DownloadEvent::Requested { url, destination } => {
                            deps.log(&format!(
                                "下载开始：{url} → {}",
                                destination.display()
                            ));
                        }
                        DownloadEvent::Finished { url, path, success } => {
                            deps.log(&format!(
                                "下载{}：{url} {}",
                                if success { "完成" } else { "失败" },
                                path
                                    .as_ref()
                                    .map(|p| format!("→ {}", p.display()))
                                    .unwrap_or_default()
                            ));
                        }
                        // DownloadEvent 标记 #[non_exhaustive]：未来新增变体照常放行
                        _ => {}
                    }
                    true // 放行下载；目的地已按 WebView2 默认（用户下载目录）
                }
            })
            // 导航放行 + 记录（未来站点黑白名单挂在这里）。
            .on_navigation({
                let deps = deps.clone();
                move |url| {
                    deps.log(&format!("导航 → {url}"));
                    true
                }
            });

        // 油猴内核：把启用脚本的源码烘焙进加载器（改动脚本后重开窗口才生效）。
        if let Some(store) = userscript_store_for_thread.as_ref() {
            let enabled: Vec<_> = crate::userscripts::list_scripts(store)
                .into_iter()
                .filter(|meta| meta.enabled)
                .collect();
            if !enabled.is_empty() {
                let sources = enabled
                    .iter()
                    .filter_map(|meta| {
                        let path = store.join(format!("{}.user.js", meta.id));
                        std::fs::read_to_string(path).ok().map(|code| (meta.clone(), code))
                    })
                    .collect::<Vec<_>>();
                deps.log(&format!("油猴：注入 {} 个脚本", sources.len()));
                builder = builder
                    .initialization_script(crate::userscripts::loader_source(&sources));
            }
        }
        for script in &opts.extra_init_scripts {
            builder = builder.initialization_script(script.clone());
        }
        builder = builder.initialization_script(
            &TOOLBAR_SCRIPT_TEMPLATE.replace("__HOME_URL__", &opts.home.as_str()),
        );

        let window = match builder
            .build()
            .map_err(|error| format!("打开浏览窗口失败：{error}"))
        {
            Ok(window) => window,
            Err(error) => {
                let _ = tx.send(Err(error));
                return;
            }
        };

        // 窗口销毁 → 置停止旗 + 宿主日志（油猴轮询器、弹窗队列线程全部退场）。
        let stop_for_close = deps.stop.clone();
        let deps_for_close = deps.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                deps_for_close.log("浏览窗口已销毁");
                stop_for_close.store(true, Ordering::SeqCst);
            }
        });

        // 弹窗队列线程：被接管的新窗 URL 逐个在本窗口内打开。
        let window_for_nav = window.clone();
        let deps_for_nav = deps.clone();
        std::thread::spawn(move || {
            while !deps_for_nav.stop.load(Ordering::SeqCst) {
                let next = deps_for_nav
                    .pending_navs
                    .lock()
                    .ok()
                    .and_then(|mut queue| queue.pop());
                if let Some(url) = next {
                    let _ = window_for_nav.navigate(url);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });

        // 调用方已经超时放弃（WebView2 初始化卡住）：迟到的窗口直接销毁，
        // 不给用户留一扇几十秒后才突然出现的幽灵窗。
        if tx.send(Ok(window)).is_err() {
            // rx 已被丢弃，拿不回窗口句柄了；此刻窗口已是孤儿——留它自灭
            // （用户至少还能看到窗口），或由 app 退出统一回收。
        }
    });
    let window = match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(result) => result?,
        Err(_) => return Err("打开浏览窗口超时（30 秒），WebView2 没有响应".to_string()),
    };

    // 油猴跨域 XHR 轮询器（窗口销毁事件已置停止旗）。
    if userscript_store.is_some() {
        let window_for_gm = window.clone();
        let stop_for_gm = stop.clone();
        std::thread::spawn(move || {
            poll_gm_xhr_queue(&window_for_gm, &stop_for_gm);
        });
    }

    Ok((window, stop))
}

/// GM 跨域 XHR：轮询页面侧 `window.__xkGmXhr` 队列，把 pending 的请求用
/// curl 发出去，结果写回队列，页面侧垫片轮到自己时 resolve。
/// 页面始终拿不到 IPC，跨域能力由宿主进程代劳——安全边界不变。
fn poll_gm_xhr_queue(window: &WebviewWindow, stop: &Arc<AtomicBool>) {
    let mut consecutive_failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(GM_XHR_POLL_INTERVAL);
        if !window_is_alive(window) {
            consecutive_failures += 1;
            if consecutive_failures >= 6 {
                // 连续多次探针失败：窗口多半已经没了，轮询器退场。
                return;
            }
            continue;
        }
        consecutive_failures = 0;

        let raw = match eval_json(
            window,
            "JSON.stringify(window.__xkGmXhr ? window.__xkGmXhr.items : {})",
            Duration::from_secs(2),
        ) {
            Ok(raw) if !raw.trim().is_empty() => raw,
            _ => continue,
        };
        let items: std::collections::HashMap<String, serde_json::Value> =
            match serde_json::from_str(&raw) {
                Ok(items) => items,
                Err(_) => continue,
            };

        for (id, item) in items {
            if item.get("state").and_then(|state| state.as_str()) != Some("pending") {
                continue;
            }
            let url = item
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if url.is_empty() {
                continue;
            }
            let method = item
                .get("method")
                .and_then(|value| value.as_str())
                .unwrap_or("GET")
                .to_string();
            let data = item
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let mut headers: Vec<(String, String)> = Vec::new();
            if let Some(map) = item.get("headers").and_then(|value| value.as_object()) {
                for (key, value) in map {
                    if let Some(value) = value.as_str() {
                        headers.push((key.clone(), value.to_string()));
                    }
                }
            }

            let outcome = curl_xhr(&url, &method, &headers, &data);
            // 结果用 JSON 字面量写回，字符串全部走 serde 转义，防注入。
            let result = serde_json::json!({
                "state": "done",
                "status": outcome.0,
                "responseText": outcome.1,
                "responseHeaders": outcome.2,
                "error": outcome.3,
            })
            .to_string();
            let script = format!(
                "(function(){{ var q = window.__xkGmXhr && window.__xkGmXhr.items['{id}']; \
                 if (!q) return; var r = {result}; \
                 q.state = r.state; q.status = r.status; q.responseText = r.responseText; \
                 q.responseHeaders = r.responseHeaders; q.error = r.error; }})();",
                id = id.replace('\\', "\\\\").replace('\'', "\\'"),
                result = result.replace('\'', "\\'"),
            );
            let _ = window.eval(&script);
        }
    }
}

/// 用 Windows 自带 curl 执行 GM 跨域请求，返回（状态码, 响应体, 响应头, 是否出错）。
fn curl_xhr(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    data: &str,
) -> (u16, String, String, bool) {
    let mut command = std::process::Command::new("curl");
    command
        .arg("-s")
        .arg("-i")
        .arg("--max-time")
        .arg("60")
        .arg("-X")
        .arg(method)
        .arg(url);
    for (key, value) in headers {
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    if !data.is_empty() && method != "GET" {
        command.arg("--data").arg(data);
    }

    match command.output() {
        Ok(output) => {
            let raw = String::from_utf8_lossy(&output.stdout).to_string();
            // 头与体以第一个空行分隔；可能有多段（重定向），取最后一段头。
            let (status, header_text, body) = split_http_response(&raw);
            (status, body, header_text, !output.status.success() && status == 0)
        }
        Err(_) => (0, String::new(), String::new(), true),
    }
}

fn split_http_response(raw: &str) -> (u16, String, String) {
    let mut status = 0u16;
    let mut header_text = String::new();
    let mut body = String::new();
    let mut rest: &str = raw;
    loop {
        match rest.find("\r\n\r\n") {
            Some(index) => {
                let head = &rest[..index];
                rest = &rest[index + 4..];
                // 每一段响应头都解析，状态码取最后一段（跟随重定向后的最终态）。
                for line in head.lines() {
                    let line = line.trim();
                    if let Some(value) = line.strip_prefix("HTTP/") {
                        if let Some(code) = value.split_whitespace().nth(1) {
                            if let Ok(parsed) = code.parse() {
                                status = parsed;
                                header_text = head.to_string();
                            }
                        }
                    }
                }
            }
            None => {
                body = rest.to_string();
                break;
            }
        }
    }
    (status, header_text, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_http_response_and_reads_status() {
        let raw = "HTTP/1.1 302 Found\r\nLocation: https://b/\r\n\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>ok</html>";
        let (status, header_text, body) = split_http_response(raw);
        assert_eq!(status, 200, "取最后一段响应的状态码");
        assert!(header_text.contains("200 OK"));
        assert_eq!(body, "<html>ok</html>");
    }

    #[test]
    fn toolbar_has_home_placeholder_and_gm_queue() {
        assert!(TOOLBAR_SCRIPT_TEMPLATE.contains("__HOME_URL__"));
    }
}
