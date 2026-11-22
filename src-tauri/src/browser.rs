//! 匣浏览器 v2 —— 匣系应用共用的内嵌浏览器引擎（多 webview 架构）。
//!
//! v1（2026-11-21 凌晨）是「单 webview + 页内 Shadow DOM 工具栏」，实测三宗罪：
//! 工具栏悬浮遮住页面内容、地址栏被每秒一次的状态同步打断（Shadow DOM 里
//! `document.activeElement` 拿到的是宿主元素，聚焦判定永远失败）、没有多标签。
//! v2 启用 tauri `unstable` 的 multiwebview API 重写：
//!
//! - 一个原生窗口 host **两条层**：顶部 `TOOLBAR_HEIGHT` 高的**工具栏 webview**
//!   （本仓 `public/browser-toolbar.html`，宿主自己的页面，样式自由、真空间占位
//!   不遮内容、可编辑地址栏）+ 其下 **N 个内容 webview**（每个标签一个，
//!   hide/show 保活切换）；
//! - 工具栏与引擎走「队列契约」（本仓安全模式：任何页面都拿不到 IPC）：
//!   工具栏写 `window.__xkReq`（动作），引擎 200ms 轮询执行并把最新状态写回
//!   `window.__xkState`；
//! - 弹窗（target=_blank / window.open）不再「Deny 后本窗打开」，而是**开新标签**；
//! - 油猴内核不变（见 userscripts.rs），每个内容 webview 都烘焙加载器，
//!   GM 跨域 XHR 轮询器遍历本浏览器全部标签。
//!
//! 嵌入指南与能力边界见 `docs/匣浏览器经验.md`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewBuilder, WebviewUrl, Window};

use crate::userscripts::{self, ScriptMeta};

/// 工具栏条的逻辑高度（像素）。内容 webview 从这条线以下开始铺。
const TOOLBAR_HEIGHT: f64 = 38.0;
/// 建窗口最多等这么久；超时说明 WebView2 初始化卡住，直接报错收场。
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
/// 工具栏请求队列 / 状态推送的轮询间隔。
const TOOLBAR_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// GM 跨域 XHR 队列的轮询间隔。
const GM_XHR_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 存活探针的单次等待。窗口活着但页面忙时可能要等一会，别太紧。
const ALIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 每个浏览器实例（一个原生窗 = 工具栏 + 若干内容标签）的状态。
struct BrowserState {
    window: Window,
    toolbar: Webview,
    /// 标签按创建顺序；展示顺序即此顺序。
    tabs: Vec<TabRecord>,
    /// 当前激活的标签 id。
    active: String,
    /// 引擎配置（extra_init / userscript_store / home / log 供新标签复用）。
    opts: BrowserOptions,
    /// 上一次推给工具栏的状态 JSON（去重，避免 200ms 空转写穿）。
    last_pushed: String,
}

struct TabRecord {
    id: String,
    webview: Webview,
    url: String,
    title: String,
    /// 该标签的页面缩放（1.0 = 100%）。
    zoom: f64,
}

static BROWSERS: Mutex<Option<HashMap<String, BrowserState>>> = Mutex::new(None);

/// 浏览器配置。宿主按需填，全部字段都有合理默认。
pub struct BrowserOptions {
    /// 窗口标题。
    pub title: String,
    /// 主页（工具栏「⌂」按钮的目标）。
    pub home: Url,
    /// 宿主自有的页面脚本（如画匣的站点解析器；注入到每个内容标签）。
    pub extra_init_scripts: Vec<String>,
    /// 油猴脚本仓库目录；None = 禁用油猴内核。
    pub userscript_store: Option<PathBuf>,
    /// 窗口初始尺寸。
    pub size: (f64, f64),
    /// 宿主日志回调（弹窗/下载/导航/油猴事件都走它）。
    pub log: Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>>,
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

/// 浏览窗口是否真的活着（页面还能执行脚本并把结果送回来）。
///
/// 「卡退」的尸体有两种形态：被关掉的（句柄已死，任何调用都报错）和渲染
/// 进程冻住的（外壳还在，navigate 等调用全部「成功」但页面永远不动）。
/// 后一种光看返回值看不出来，必须发脚本探针并等回调真的回来。
pub fn window_is_alive(window: &Webview) -> bool {
    matches!(
        eval_json(window, "1", ALIVE_PROBE_TIMEOUT),
        Ok(value) if value.trim() == "1"
    )
}

/// 浏览器是否存活（存在 + 激活标签响应探针）。
pub fn is_alive(label: &str) -> bool {
    match active_webview(label) {
        Ok(webview) => window_is_alive(&webview),
        Err(_) => false,
    }
}

/// 取浏览器的激活标签（抓取流直接拿它来 eval/导航）。
pub fn active_webview(label: &str) -> Result<Webview, String> {
    with_state(label, |state| {
        state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .map(|tab| tab.webview.clone())
            .ok_or_else(|| "浏览器没有激活标签".to_string())
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

fn with_state<R>(label: &str, f: impl FnOnce(&mut BrowserState) -> R) -> Option<R> {
    let mut guard = BROWSERS.lock().ok()?;
    let map = guard.as_mut()?;
    let state = map.get_mut(label)?;
    Some(f(state))
}

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

/// 打开一个新的浏览器窗口（工具栏 + 首个标签）。
///
/// 返回窗口 label（调用方存好，后续所有命令都按 label 路由）。
/// 已存在同名浏览器时报错——宿主应先用 [`browser_exists`] 判断。
pub fn open(
    app: &AppHandle,
    url: Url,
    label: String,
    opts: BrowserOptions,
) -> Result<String, String> {
    if browser_exists(&label) {
        return Err("该浏览器窗口已在运行".to_string());
    }

    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_for_window = app.clone();
    let userscript_store = opts.userscript_store.clone();
    let label_for_thread = label.clone();
    let first_tab_id = format!("{label}-tab-0");
    // 建窗整体放到独立线程（add_child 在同步命令/事件回调里调用会死锁，
    // tauri 文档明说；线程 + channel 是官方推荐姿势）。
    std::thread::spawn(move || {
        let label = label_for_thread;
        let run = || -> Result<(), String> {
            let window = tauri::WindowBuilder::new(&app_for_window, &label)
                .title(opts.title.clone())
                .inner_size(opts.size.0, opts.size.1)
                .build()
                .map_err(|error| format!("打开浏览窗口失败：{error}"))?;

            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size().unwrap_or_default();
            let width = inner.width as f64 / scale;
            let height = inner.height as f64 / scale;

            // 工具栏：本仓公共页（真实占位，不遮内容；与引擎走队列契约）。
            let toolbar = window
                .add_child(
                    WebviewBuilder::new(
                        format!("{label}-toolbar"),
                        WebviewUrl::App("browser-toolbar.html".into()),
                    ),
                    LogicalPosition::new(0.0, 0.0),
                    LogicalSize::new(width, TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建工具栏失败：{error}"))?;

            let tab_url = url.to_string();
            let mut tab_builder = WebviewBuilder::new(
                first_tab_id.clone(),
                WebviewUrl::External(url),
            )
            .auto_resize();
            tab_builder = attach_tab_hooks(tab_builder, &label, &first_tab_id);
            if let Some(store) = userscript_store.as_ref() {
                tab_builder = attach_userscripts(tab_builder, store, &label);
            }
            for script in &opts.extra_init_scripts {
                tab_builder = tab_builder.initialization_script(script.clone());
            }

            let webview = window
                .add_child(
                    tab_builder,
                    LogicalPosition::new(0.0, TOOLBAR_HEIGHT),
                    LogicalSize::new(width, height - TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建内容标签失败：{error}"))?;

            let state = BrowserState {
                window: window.clone(),
                toolbar,
                tabs: vec![TabRecord {
                    id: first_tab_id.clone(),
                    webview,
                    url: tab_url,
                    title: String::new(),
                    zoom: 1.0,
                }],
                active: first_tab_id,
                opts,
                last_pushed: String::new(),
            };
            register_browser(&label, state, &app_for_window);
            Ok(())
        };
        let _ = tx.send(run());
    });
    match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(result) => result?,
        Err(_) => return Err("打开浏览窗口超时（30 秒），WebView2 没有响应".to_string()),
    }

    Ok(label)
}

/// 浏览器窗口是否存活（存在 + 激活标签探针通过）。
pub fn browser_exists(label: &str) -> bool {
    BROWSERS
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|map| map.contains_key(label))
        })
        .unwrap_or(false)
}

/// 当前浏览器的激活内容 webview（抓取流直接拿它 eval/轮询）。
pub fn active_webview_of(label: &str) -> Result<Webview, String> {
    active_webview(label)
}

/// 激活标签导航到指定地址。
pub fn navigate_active(label: &str, url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|e| format!("网址格式不正确: {e}"))?;
    with_state(label, |state| {
        let tab = state
            .tabs
            .iter_mut()
            .find(|tab| tab.id == state.active)
            .ok_or("浏览器没有激活标签")?;
        tab.webview
            .navigate(parsed)
            .map_err(|e| format!("导航失败: {e}"))
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

/// 后退/前进/刷新（子 Webview 没有历史 API，走页面 eval）。
pub fn history_action(label: &str, action: &str) -> Result<(), String> {
    let js = match action {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        _ => return Err("未知的历史动作".to_string()),
    };
    with_state(label, |state| {
        let tab = state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .ok_or("浏览器没有激活标签")?;
        tab.webview
            .eval(js)
            .map_err(|e| format!("执行失败: {e}"))
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

/// 新建标签（弹窗接管与工具栏「+」共用）。返回标签 id。
pub fn new_tab(app: &AppHandle, label: &str, url: &str, activate: bool) -> Result<String, String> {
    let parsed = Url::parse(url).map_err(|e| format!("网址格式不正确: {e}"))?;
    let tab_id = format!("{label}-tab-{}", uuid::Uuid::new_v4().simple());
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_for_tab = app.clone();
    let label_for_tab = label.to_string();
    let tab_id_for_thread = tab_id.clone();
    let url_for_thread = parsed.clone();
    let activate_for_thread = activate;

    // add_child 不能在同步命令/事件回调里跑（tauri 明说会死锁），线程 + channel。
    std::thread::spawn(move || {
        let run = || -> Result<(), String> {
            let (window, extra_init, store) = with_state(&label_for_tab, |state| {
                (
                    state.window.clone(),
                    state.opts.extra_init_scripts.clone(),
                    state.opts.userscript_store.clone(),
                )
            })
            .ok_or("浏览器不存在")?;

            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size().unwrap_or_default();
            let width = inner.width as f64 / scale;
            let height = inner.height as f64 / scale;

            let mut tab_builder = WebviewBuilder::new(
                tab_id_for_thread.clone(),
                WebviewUrl::External(url_for_thread.clone()),
            )
            .auto_resize();
            tab_builder = attach_tab_hooks(tab_builder, &label_for_tab, &tab_id_for_thread);
            if let Some(store) = store.as_ref() {
                tab_builder = attach_userscripts(tab_builder, store, &label_for_tab);
            }
            for script in &extra_init {
                tab_builder = tab_builder.initialization_script(script.clone());
            }
            let webview = window
                .add_child(
                    tab_builder,
                    LogicalPosition::new(0.0, TOOLBAR_HEIGHT),
                    LogicalSize::new(width, height - TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建标签失败：{error}"))?;

            let tab_url = url_for_thread.to_string();
            with_state(&label_for_tab, |state| {
                let mut record = TabRecord {
                    id: tab_id_for_thread.clone(),
                    webview,
                    url: tab_url,
                    title: String::new(),
                    zoom: 1.0,
                };
                if activate_for_thread {
                    for tab in &state.tabs {
                        let _ = tab.webview.hide();
                    }
                    state.tabs.push(record);
                    state.active = tab_id_for_thread.clone();
                } else {
                    // 后台标签：先藏起来，激活的还是原来的
                    let _ = record.webview.hide();
                    state.tabs.push(record);
                }
                Ok::<(), String>(())
            })
            .ok_or("浏览器不存在")??;
            push_state(&label_for_tab);
            Ok(())
        };
        let _ = tx.send(run());
    });
    match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(result) => result?,
        Err(_) => return Err("创建标签超时（30 秒）".to_string()),
    }
    if activate {
        let _ = focus_tab_webview(&tab_id);
    }
    Ok(tab_id)
}

/// 切换标签：全部隐藏后显示目标（隐藏的标签保持进程与状态存活）。
pub fn switch_tab(label: &str, tab_id: &str) -> Result<(), String> {
    with_state(label, |state| {
        for tab in &state.tabs {
            if tab.id == tab_id {
                let _ = tab.webview.show();
                let _ = tab.webview.set_focus();
            } else {
                let _ = tab.webview.hide();
            }
        }
        state.active = tab_id.to_string();
        Ok::<(), String>(())
    })
    .ok_or_else(|| "浏览器不存在".to_string())??;
    push_state(label);
    Ok(())
}

/// 关闭标签；最后一个标签关闭时整个浏览器窗口一起关。
pub fn close_tab(label: &str, tab_id: &str) -> Result<(), String> {
    let close_window: bool = with_state(label, |state| {
        let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let tab = state.tabs.remove(index);
        let _ = tab.webview.close();
        if state.active == tab_id {
            state.active = state
                .tabs
                .last()
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
            if let Some(active) = state.tabs.iter().find(|tab| tab.id == state.active) {
                let _ = active.webview.show();
            }
        }
        state.tabs.is_empty()
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;

    if close_window {
        close_browser(label)?;
    } else {
        push_state(label);
    }
    Ok(())
}

/// 关闭整个浏览器窗口（状态在销毁事件里清理）。
pub fn close_browser(label: &str) -> Result<(), String> {
    with_state(label, |state| state.window.close())
        .ok_or_else(|| "浏览器不存在".to_string())?
        .map_err(|e| format!("关闭浏览器失败: {e}"))
}

/// 激活标签缩放（factor = 1.0 为 100%）。
pub fn set_zoom(label: &str, factor: f64) -> Result<(), String> {
    let factor = factor.clamp(0.25, 4.0);
    with_state(label, |state| {
        let tab = state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .ok_or("浏览器没有激活标签")?;
        tab.webview
            .set_zoom(factor)
            .map_err(|e| format!("缩放失败: {e}"))
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

fn focus_tab_webview(tab_id: &str) -> Result<(), String> {
    let mut guard = BROWSERS.lock().map_err(|_| "浏览器状态异常".to_string())?;
    let map = guard.as_mut().ok_or("浏览器状态异常")?;
    for state in map.values_mut() {
        if let Some(tab) = state.tabs.iter().find(|tab| tab.id == tab_id) {
            return tab
                .webview
                .set_focus()
                .map_err(|e| format!("聚焦失败: {e}"));
        }
    }
    Ok(())
}

/// 给内容标签挂钩子：弹窗→开新标签、下载→宿主日志、导航→记 URL。
fn attach_tab_hooks(
    builder: tauri::WebviewBuilder<tauri::Wry>,
    label: &str,
    tab_id: &str,
) -> tauri::WebviewBuilder<tauri::Wry> {
    let label_for_new = label.to_string();
    let label_for_dl = label.to_string();
    let label_for_nav = label.to_string();
    let tab_id_for_nav = tab_id.to_string();
    builder
        // 弹窗接管 v2：新窗一律开成新标签（后台建，建好即激活）。
        .on_new_window(move |new_url, _features| {
            queue_pending_tab(&label_for_new, &new_url.to_string(), true);
            NewWindowResponse::Deny
        })
        .on_download({
            let label_for_download = label_for_dl.clone();
            move |_webview, event| {
                match event {
                    DownloadEvent::Requested { url, destination } => {
                        log_for(
                            &label_for_download,
                            &format!("下载开始：{url} → {}", destination.display()),
                        );
                    }
                    DownloadEvent::Finished { url, path, success } => {
                        log_for(
                            &label_for_download,
                            &format!(
                                "下载{}：{url} {}",
                                if success { "完成" } else { "失败" },
                                path.as_ref()
                                    .map(|p| format!("→ {}", p.display()))
                                    .unwrap_or_default()
                            ),
                        );
                    }
                    // DownloadEvent 标记 #[non_exhaustive]
                    _ => {}
                }
                true // 放行下载；目的地按 WebView2 默认（用户下载目录）
            }
        })
        .on_navigation({
            let label_for_navigation = label_for_nav.clone();
            let tab_id_for_navigation = tab_id_for_nav.clone();
            move |url| {
                note_tab_url(&label_for_navigation, &tab_id_for_navigation, &url.to_string());
                true
            }
        })
        // 右键菜单（引擎级）：页面 preventDefault 掉 WebView2 原生菜单后，
        // 由本脚本画匣系自己的菜单。
        .initialization_script(CONTEXT_MENU_SCRIPT)
}

/// 挂油猴加载器（含日志），返回追加后的 builder。
fn attach_userscripts(
    builder: tauri::WebviewBuilder<tauri::Wry>,
    store: &PathBuf,
    label: &str,
) -> tauri::WebviewBuilder<tauri::Wry> {
    let enabled: Vec<ScriptMeta> = userscripts::list_scripts(store)
        .into_iter()
        .filter(|meta| meta.enabled)
        .collect();
    if enabled.is_empty() {
        return builder;
    }
    let sources = enabled
        .iter()
        .filter_map(|meta| {
            let path = store.join(format!("{}.user.js", meta.id));
            std::fs::read_to_string(path).ok().map(|code| (meta.clone(), code))
        })
        .collect::<Vec<_>>();
    log_for(label, &format!("油猴：注入 {} 个脚本", sources.len()));
    builder.initialization_script(userscripts::loader_source(&sources))
}


/// 内容页右键菜单脚本（引擎级，所有内容标签注入）。
///
/// WebView2 的规则：页面在 `contextmenu` 事件里 `preventDefault()`，原生默认
/// 菜单就不弹——所以纯页内实现即可。菜单本体也是 Shadow DOM（不沾站点样式）。
/// 本地动作（复制/后退/刷新）页面内完成；需要引擎的动作（开新标签）写
/// `window.__xkReq` 队列，由引擎轮询器消化。
const CONTEXT_MENU_SCRIPT: &str = r###"
(function () {
  if (window.__xkCtxMenu) return;
  window.__xkCtxMenu = true;
  var host = null;

  function closeMenu() {
    if (host) { host.remove(); host = null; }
    document.removeEventListener("click", closeMenu, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", closeMenu, true);
  }
  function onKey(e) { if (e.key === "Escape") closeMenu(); }

  function item(label, fn, disabled) {
    var it = document.createElement("div");
    it.className = "xm-item";
    it.textContent = label;
    if (disabled) it.className += " xm-disabled";
    else it.addEventListener("click", function (e) { e.stopPropagation(); closeMenu(); fn(); });
    return it;
  }
  function sep() { var s = document.createElement("div"); s.className = "xm-sep"; return s; }

  function copyText(text) {
    if (!text) return;
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).catch(function () { legacyCopy(text); });
    } else legacyCopy(text);
  }
  function legacyCopy(text) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed"; ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); } catch (e) {}
    ta.remove();
  }

  function openMenu(x, y, target) {
    closeMenu();
    var link = target.closest ? target.closest("a") : null;
    var img = target.tagName === "IMG" ? target : (target.closest ? target.closest("img") : null);
    var selection = String(window.getSelection ? window.getSelection() : "");
    var items = [];

    if (link) {
      items.push(item("在新标签打开链接", function () { window.__xkReq = { action: "new", url: link.href }; }));
      items.push(item("复制链接地址", function () { copyText(link.href); }));
    }
    if (img && (img.currentSrc || img.src)) {
      items.push(item("在新标签打开图片", function () { window.__xkReq = { action: "new", url: img.currentSrc || img.src }; }));
      items.push(item("复制图片地址", function () { copyText(img.currentSrc || img.src); }));
      items.push(item("打开原图并另存", function () {
        var url = img.currentSrc || img.src;
        var a = document.createElement("a");
        a.href = url; a.download = "";
        document.body.appendChild(a); a.click(); a.remove();
      }));
    }
    if (selection) items.push(item("复制选中文本", function () { copyText(selection); }));
    items.push(sep());
    items.push(item("后退", function () { history.back(); }));
    items.push(item("前进", function () { history.forward(); }));
    items.push(item("刷新", function () { location.reload(); }));
    items.push(sep());
    items.push(item("全选", function () {
      var range = document.createRange();
      range.selectNodeContents(document.body);
      var sel = window.getSelection();
      if (sel) { sel.removeAllRanges(); sel.addRange(range); }
    }));

    host = document.createElement("div");
    host.setAttribute("style", "position:fixed;inset:0;z-index:2147483646;");
    var root = host.attachShadow ? host.attachShadow({ mode: "open" }) : host;
    var style = document.createElement("style");
    style.textContent = [
      ".xm{position:fixed;min-width:180px;padding:5px;border-radius:10px;",
      "background:rgba(20,22,27,.98);border:1px solid #333842;",
      "box-shadow:0 8px 24px rgba(0,0,0,.45);color:#e8eaf0;",
      "font:12px 'Segoe UI','Microsoft YaHei UI',sans-serif;}",
      ".xm-item{padding:7px 12px;border-radius:6px;cursor:pointer;}",
      ".xm-item:hover{background:#2c313c;}",
      ".xm-disabled{color:#5a6070;cursor:default;}",
      ".xm-disabled:hover{background:transparent;}",
      ".xm-sep{height:1px;margin:4px 8px;background:#333842;}",
    ].join("");
    root.appendChild(style);
    var menu = document.createElement("div");
    menu.className = "xm";
    for (var i = 0; i < items.length; i++) menu.appendChild(items[i]);
    root.appendChild(menu);
    document.documentElement.appendChild(host);

    // 视口内收放
    var rect = menu.getBoundingClientRect();
    var maxX = window.innerWidth - rect.width - 4;
    var maxY = window.innerHeight - rect.height - 4;
    menu.style.left = Math.min(x, Math.max(0, maxX)) + "px";
    menu.style.top = Math.min(y, Math.max(0, maxY)) + "px";

    setTimeout(function () {
      document.addEventListener("click", closeMenu, true);
      document.addEventListener("keydown", onKey, true);
      window.addEventListener("blur", closeMenu, true);
      // 右键另一处：换菜单不关壳
      document.addEventListener("contextmenu", function (e) {
        if (host) { e.preventDefault(); }
      }, true);
    }, 0);
  }

  document.addEventListener("contextmenu", function (e) {
    e.preventDefault();
    openMenu(e.clientX, e.clientY, e.target);
  }, false);
})();
"###;

/// 待建标签的进程级队列（弹窗/工具栏入口都往里塞，消化线程统一建）。
static PENDING_TABS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn queue_pending_tab(label: &str, url: &str, activate: bool) {
    if let Ok(mut queue) = PENDING_TABS.lock() {
        // activate 目前恒为 true：弹窗总是切过去看；静默标签走 new_tab 显式调用。
        let _ = activate;
        queue.push((label.to_string(), url.to_string()));
    }
}

/// 消化待建标签的线程（每个浏览器实例一份）。
fn start_pending_tab_worker(app: &AppHandle, label: &str, stop: Arc<AtomicBool>) {
    let label = label.to_string();
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if !browser_exists(&label) {
                return;
            }
            let next = PENDING_TABS
                .lock()
                .ok()
                .and_then(|mut queue| {
                    let index = queue.iter().position(|(queued_label, _)| queued_label == &label)?;
                    Some(queue.remove(index))
                });
            if let Some((_, url)) = next {
                if let Err(error) = new_tab(&app, &label, &url, true) {
                    log_for(&label, &format!("弹窗开标签失败：{error}"));
                }
            } else {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

fn register_browser(label: &str, state: BrowserState, app: &AppHandle) {
    // 窗口销毁 → 状态表里摘除（轮询器靠 browser_exists 自行退场）。
    let label_for_close = label.to_string();
    state.window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Ok(mut guard) = BROWSERS.lock() {
                if let Some(map) = guard.as_mut() {
                    map.remove(&label_for_close);
                }
            }
        }
    });
    if let Ok(mut guard) = BROWSERS.lock() {
        if let Some(map) = guard.as_mut() {
            map.insert(label.to_string(), state);
        }
    }
    // 重算布局（窗口尺寸可能在建标签期间变了）。
    reposition(label);
    // 引擎轮询器：工具栏请求 + 待建标签消化 + 油猴跨域 XHR。
    start_toolbar_poller(label, app.clone());
    start_pending_tab_worker(app, label, Arc::new(AtomicBool::new(false)));
    start_gm_poller(label, app.clone());
}

/// 工具栏请求轮询器：读 `__xkReq` 执行动作，把最新状态写回 `__xkState`。
fn start_toolbar_poller(label: &str, app: AppHandle) {
    let label = label.to_string();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(TOOLBAR_POLL_INTERVAL);
            if !browser_exists(&label) {
                return;
            }
            let Some(toolbar) = toolbar_of(&label) else {
                return;
            };
            let raw = match eval_json(
                &toolbar,
                "JSON.stringify(window.__xkReq || null)",
                Duration::from_secs(2),
            ) {
                Ok(raw) if !raw.trim().is_empty() && raw != "null" => raw,
                _ => {
                    push_state(&label);
                    continue;
                }
            };
            let req: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let action = req
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let arg = req
                .get("url")
                .or_else(|| req.get("tabId"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let result: Result<(), String> = match action.as_str() {
                "nav" if !arg.is_empty() => {
                    let target = if arg.starts_with("http://") || arg.starts_with("https://") {
                        arg.clone()
                    } else {
                        format!("https://{arg}")
                    };
                    navigate_active(&label, &target)
                }
                "back" => history_action(&label, "back"),
                "forward" => history_action(&label, "forward"),
                "reload" => history_action(&label, "reload"),
                "home" => {
                    let home = with_state(&label, |state| state.opts.home.to_string())
                        .unwrap_or_default();
                    if home.is_empty() {
                        Ok(())
                    } else {
                        navigate_active(&label, &home)
                    }
                }
                "new" => new_tab(&app, &label, &arg, true).map(|_| ()),
                "close" => close_tab(&label, &arg),
                "switch" => switch_tab(&label, &arg),
                "zoomIn" => adjust_zoom(&label, 0.1),
                "zoomOut" => adjust_zoom(&label, -0.1),
                "zoomReset" => set_zoom(&label, 1.0),
                "zoomReset" => set_zoom(&label, 1.0),
                _ => Ok(()),
            };
            if let Err(error) = result {
                log_for(&label, &format!("工具栏动作 {action} 失败：{error}"));
            }
            // 清掉请求槽（先清再推状态，避免重复执行）。
            let _ = toolbar.eval("window.__xkReq = null;");
            push_state(&label);
        }
    });
}

fn adjust_zoom(label: &str, delta: f64) -> Result<(), String> {
    with_state(label, |state| {
        let tab = state
            .tabs
            .iter_mut()
            .find(|tab| tab.id == state.active)
            .ok_or("浏览器没有激活标签")?;
        tab.zoom = (tab.zoom + delta).clamp(0.25, 4.0);
        let factor = tab.zoom;
        tab.webview
            .set_zoom(factor)
            .map_err(|e| format!("缩放失败: {e}"))
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

fn toolbar_of(label: &str) -> Option<Webview> {
    with_state(label, |state| state.toolbar.clone())
}

/// 油猴跨域 XHR 轮询器：遍历本浏览器全部标签的 `__xkGmXhr` 队列。
fn start_gm_poller(label: &str, app: AppHandle) {
    let label = label.to_string();
    std::thread::spawn(move || {
        let _ = app;
        loop {
            std::thread::sleep(GM_XHR_POLL_INTERVAL);
            let Some(webviews) = tab_webviews(&label) else {
                return;
            };
            if webviews.is_empty() {
                return;
            }
            for webview in &webviews {
                poll_tab_req(&label, webview);
                poll_gm_xhr_queue(webview);
            }
        }
    });
}

fn tab_webviews(label: &str) -> Option<Vec<Webview>> {
    with_state(label, |state| {
        state.tabs.iter().map(|tab| tab.webview.clone()).collect()
    })
}

fn app_of(label: &str) -> Option<AppHandle> {
    with_state(label, |state| state.window.app_handle().clone())
}

/// 内容页右键菜单的「新标签」请求：与工具栏同一套动作分派。
fn poll_tab_req(label: &str, webview: &Webview) {
    let raw = match eval_json(
        webview,
        "JSON.stringify(window.__xkReq || null)",
        Duration::from_secs(2),
    ) {
        Ok(raw) if !raw.trim().is_empty() && raw != "null" => raw,
        _ => return,
    };
    let req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return,
    };
    if req.get("action").and_then(|value| value.as_str()) != Some("new") {
        return;
    }
    let Some(url) = req.get("url").and_then(|value| value.as_str()) else {
        return;
    };
    if url.is_empty() {
        return;
    }
        let Some(app) = app_of(label) else {
        return;
    };
    match new_tab(&app, label, url, true) {
        Ok(_) => {
            let _ = webview.eval("window.__xkReq = null;");
        }
        Err(error) => log_for(label, &format!("右键新标签失败：{error}")),
    }
}

/// 油猴跨域 XHR：轮询页面侧 `window.__xkGmXhr` 队列，把 pending 的请求用
/// curl 发出去，结果写回队列，页面侧垫片轮到自己时 resolve。
/// 页面始终拿不到 IPC，跨域能力由宿主进程代劳——安全边界不变。
fn poll_gm_xhr_queue(webview: &Webview) {
    if !window_is_alive(webview) {
        return;
    }
    let raw = match eval_json(
        webview,
        "JSON.stringify(window.__xkGmXhr ? window.__xkGmXhr.items : {})",
        Duration::from_secs(2),
    ) {
        Ok(raw) if !raw.trim().is_empty() && raw != "{}" => raw,
        _ => return,
    };
    let items: HashMap<String, serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(items) => items,
        Err(_) => return,
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
        let _ = webview.eval(&script);
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

fn log_for(label: &str, message: &str) {
    if let Some(log) =
        with_state(label, |state| state.opts.log.clone()).flatten()
    {
        log(message);
    }
}

fn note_tab_url(label: &str, tab_id: &str, url: &str) {
    with_state(label, |state| {
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.url = url.to_string();
        }
    });
    push_state(label);
}

/// 把最新状态（标签列表/激活项/当前 URL）写进工具栏页。
fn push_state(label: &str) {
    let Some(toolbar) = toolbar_of(label) else {
        return;
    };
    let state_json = with_state(label, |state| {
        let tabs: Vec<serde_json::Value> = state
            .tabs
            .iter()
            .map(|tab| {
                serde_json::json!({
                    "id": tab.id,
                    "title": if tab.title.is_empty() { tab.url.clone() } else { tab.title.clone() },
                    "url": tab.url,
                    "active": tab.id == state.active,
                })
            })
            .collect();
        let active_url = state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .map(|tab| tab.url.clone())
            .unwrap_or_default();
        serde_json::json!({ "label": label, "tabs": tabs, "activeUrl": active_url }).to_string()
    })
    .unwrap_or_else(|| "{}".to_string());
    let unchanged = with_state(label, |state| {
        if state.last_pushed == state_json {
            true
        } else {
            state.last_pushed = state_json.clone();
            false
        }
    })
    .unwrap_or(true);
    if unchanged {
        return;
    }
    let _ = toolbar.eval(&format!("window.__xkState = {state_json};"));
}

/// 窗口尺寸变化时重排工具栏与全部内容标签。
fn reposition(label: &str) {
    with_state(label, |state| {
        let scale = state.window.scale_factor().unwrap_or(1.0);
        let Ok(inner) = state.window.inner_size() else {
            return;
        };
        let width = inner.width as f64 / scale;
        let height = inner.height as f64 / scale;
        let _ = state.toolbar.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = state
            .toolbar
            .set_size(LogicalSize::new(width, TOOLBAR_HEIGHT));
        for tab in &state.tabs {
            let _ = tab
                .webview
                .set_position(LogicalPosition::new(0.0, TOOLBAR_HEIGHT));
            let _ = tab
                .webview
                .set_size(LogicalSize::new(width, height - TOOLBAR_HEIGHT));
        }
    });
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
}
