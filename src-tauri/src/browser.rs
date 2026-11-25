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
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewBuilder, WebviewUrl, Window};

use crate::userscripts::{self, ScriptMeta};

/// 工具栏条的逻辑高度（像素）。内容 webview 从这条线以下开始铺。
const TOOLBAR_HEIGHT: f64 = 38.0;

/// 特殊「主页」标记：工具栏 ⌂ 看到它不导航，改为发事件让主窗把该标签切回画匣新页。
/// 画匣新页是主窗里的 React 组件，webview 没法直接显示（管理员 2026-11-25：⌂ 不该回 e-hentai）。
pub const HUAXIA_NEWTAB: &str = "huaxia:newtab";
/// 建窗口最多等这么久；超时说明 WebView2 初始化卡住，直接报错收场。
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
/// 工具栏请求队列 / 状态推送的轮询间隔。
const TOOLBAR_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// 浏览器用户设置（工具栏 ⚙ 面板改，落盘 `browser/settings.json`）。
///
/// 字段用 camelCase 直接跟工具栏页对接（那边是原生 JS，不写转换层）。
/// `#[serde(default)]` + 每个字段的默认值保证旧文件/缺字段都能读。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserSettings {
    /// 主页（⌂ 按钮、「新建标签 = 主页」用它）。
    pub home: String,
    /// 新建标签行为：`home`（主页）或 `blank`（空白页）。
    pub new_tab: String,
    /// 地址栏输入不是网址时的搜索模板，`{q}` 是占位符。
    pub search_url: String,
    /// 缩放步进（每次点 ± 变化多少）。
    pub zoom_step: f64,
    /// 内容页快捷键（Ctrl+T/W/R/L、Alt+←/→、F5）。
    pub shortcuts: bool,
    /// 标签芯片显示页面标题（false 时只显示域名，窄窗口更整齐）。
    pub show_titles: bool,
    /// 标签栏最大宽度占比（0.2–0.7）。
    pub tabs_width: f64,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            home: "https://e-hentai.org/".to_string(),
            new_tab: "home".to_string(),
            search_url: "https://www.bing.com/search?q={q}".to_string(),
            zoom_step: 0.1,
            shortcuts: true,
            show_titles: true,
            tabs_width: 0.44,
        }
    }
}

impl BrowserSettings {
    /// 收敛非法值：缩放在 25%–400%，步进不低于 5%，避免面板写坏后按钮失灵。
    fn sanitized(mut self) -> Self {
        if self.home.trim().is_empty() {
            self.home = BrowserSettings::default().home;
        }
        if self.new_tab != "blank" {
            self.new_tab = "home".to_string();
        }
        if self.search_url.trim().is_empty() || !self.search_url.contains("{q}") {
            self.search_url = BrowserSettings::default().search_url;
        }
        self.zoom_step = self.zoom_step.clamp(0.05, 0.5);
        self.tabs_width = self.tabs_width.clamp(0.2, 0.7);
        self
    }

    /// 地址栏里「不是网址」的输入 → 搜索 URL。
    fn search_url_for(&self, query: &str) -> String {
        self.search_url.replace("{q}", &urlencode(query))
    }
}

/// 最小 URL 编码（只处理 query 里常见的保留字符，够搜索用）。
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `%LOCALAPPDATA%\com.archivegallery.desktop\browser\`（与 adblock.txt 同目录）。
fn browser_dir() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    let dir = PathBuf::from(base)
        .join("com.archivegallery.desktop")
        .join("browser");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 读设置，`fallback_home` 只在「settings.json 还不存在」时作为主页。
///
/// 这样第一次打开会沿用宿主（D2 面板「浏览器设置」）填的主页，等用户在
/// 工具栏 ⚙ 里保存过一次，就以文件为准——两处设置不再各说各话。
pub fn load_settings_with_fallback(fallback_home: Option<&str>) -> BrowserSettings {
    let Some(path) = browser_dir().map(|dir| dir.join("settings.json")) else {
        return BrowserSettings::default();
    };
    let exists = path.exists();
    let mut settings = if exists {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BrowserSettings>(&raw).ok())
            .unwrap_or_default()
    } else {
        BrowserSettings::default()
    };
    if !exists {
        if let Some(home) = fallback_home.map(str::trim).filter(|home| !home.is_empty()) {
            settings.home = home.to_string();
        }
    }
    settings.sanitized()
}

/// 写设置（工具栏 ⚙ 面板的「保存」走这里）。
pub fn save_settings(settings: &BrowserSettings) -> Result<(), String> {
    let path = browser_dir()
        .map(|dir| dir.join("settings.json"))
        .ok_or_else(|| "取不到本地数据目录".to_string())?;
    let raw = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("序列化浏览器设置失败：{error}"))?;
    std::fs::write(path, raw).map_err(|error| format!("写浏览器设置失败：{error}"))
}
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
    /// 用户设置（工具栏 ⚙ 面板可改，落盘 settings.json）。
    settings: BrowserSettings,
    /// ⚙ 设置浮层的 webview（打开时存在；工具栏只有 38px 高，装不下表单，
    /// 所以另开一条铺满窗口的 webview 画遮罩式面板）。
    settings_view: Option<Webview>,
    /// 上一次推给工具栏的状态 JSON（去重，避免 200ms 空转写穿）。
    last_pushed: String,
    /// 内置模式（P0 探针）：宿主不是独立窗口而是 main 窗，
    /// 摆位跟随 React 占位矩形（embed_rect）而非整窗。
    embedded: bool,
    /// 内置占位矩形（逻辑像素 x, y, w, h）；None = 前端还没同步过，先不摆位。
    embed_rect: Option<(f64, f64, f64, f64)>,
}

struct TabRecord {
    id: String,
    webview: Webview,
    url: String,
    title: String,
    /// 该标签的页面缩放（1.0 = 100%）。
    zoom: f64,
    /// 正在加载（工具栏用来转刷新按钮/地址栏左侧指示）。
    loading: bool,
}

/// 所有存活浏览器实例的状态表。
///
/// 2026-11-24 修（**致命**）：这里原来是 `Mutex<Option<HashMap>>` + `Mutex::new(None)`，
/// 而全仓没有任何一处把它写成 `Some(..)`——`register_browser` 的 insert 落在
/// `None` 上被静默丢掉。后果是整台引擎没有状态机，且完全不报错：
/// - 工具栏轮询器第一轮 `browser_exists == false` 直接 return → 顶端按钮全哑、
///   标签条空白（没有状态推送）；
/// - GM 跨域轮询器同样退场 → 织机所有经 GM_xmlhttpRequest 的图片请求没人接，
///   全部等到自己超时 → 满屏 `Error: timeout`、「时延爆表」；
/// - 页面 webview 本身照常显示，所以从外面看像「按钮坏了」而不是「引擎死了」。
///
/// 改用 `LazyLock` 后不可能再忘记初始化。
static BROWSERS: std::sync::LazyLock<Mutex<HashMap<String, BrowserState>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

// ── 诊断：面包屑 + 主线程心跳看门狗 ─────────────────────────
//
// 2026-11-24 早上真机「打开网页后直接卡死、白屏」：日志只到「正在打开浏览窗口…」
// 就没了，进程 Responding=False 挂了几小时。GUI 卡死这种事故光看日志无法定位，
// 所以这里放两件廉价工具（不参与业务逻辑）：
//
// - `crumb()`：任何线程都能塞一句「我现在在哪」，环形保留最后 24 条；
// - 看门狗：每秒让主线程设一次心跳，连续两次没设上就判定主线程卡死，
//   把最后 24 条面包屑写进 d2.log —— 卡在哪一行、卡前最后发生的事一目了然。

static CRUMBS: std::sync::LazyLock<Mutex<std::collections::VecDeque<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::VecDeque::new()));
static WATCHDOG_ON: AtomicBool = AtomicBool::new(false);
static MAIN_BEAT: AtomicU64 = AtomicU64::new(0);

/// 记一句面包屑（环形 24 条）。开销是一次短锁，只在关键步骤调用。
pub fn crumb(text: impl Into<String>) {
    let Ok(mut queue) = CRUMBS.lock() else {
        return;
    };
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64 % 1_000_000)
        .unwrap_or(0);
    queue.push_back(format!(
        "{}.{:03} {}",
        millis / 1000,
        millis % 1000,
        text.into()
    ));
    while queue.len() > 24 {
        queue.pop_front();
    }
}

fn crumbs() -> String {
    CRUMBS
        .lock()
        .map(|queue| queue.iter().cloned().collect::<Vec<_>>().join(" ← "))
        .unwrap_or_default()
}

/// 直接写 d2 日志：不依赖浏览器状态表，主线程卡死时也能落盘。
fn diag(app: &AppHandle, message: &str) {
    let Ok(cache) = app.path().app_cache_dir() else {
        return;
    };
    let dir = cache.join("d2");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("d2.log"))
    {
        use std::io::Write as _;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{stamp}] {message}");
    }
}

/// 主线程心跳看门狗（进程内只起一次）。
pub fn start_watchdog(app: &AppHandle) {
    if WATCHDOG_ON.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let base = std::time::Instant::now();
        loop {
            let before = MAIN_BEAT.load(Ordering::Relaxed);
            let _ = app.run_on_main_thread(move || {
                MAIN_BEAT.store(base.elapsed().as_millis() as u64, Ordering::Relaxed);
            });
            std::thread::sleep(Duration::from_millis(1_600));
            let after = MAIN_BEAT.load(Ordering::Relaxed);
            if after == before {
                diag(
                    &app,
                    &format!(
                        "⚠ 主线程已 {}ms 没响应；最后面包屑：{}",
                        base.elapsed().as_millis(),
                        crumbs()
                    ),
                );
            }
        }
    });
}

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
    /// 广告拦截开关（管理员 11-23；标签页创建时读一次）。
    pub adblock: bool,
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
            adblock: true,
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

/// 找出任意一个还活着的浏览窗口。
///
/// 用途：d2 侧只记了一个 label（而且只有面板那条路径会写），从别的入口
/// 打开浏览器时那个记录是空的或过期的旧值，于是所有按 label 路由的命令
/// 全部报「浏览器不存在」。这里扫一遍真实存活的窗口做兜底。
pub fn first_alive_label() -> Option<String> {
    let mut guard = BROWSERS.lock().ok()?;
    let map = &mut *guard;
    let labels: Vec<String> = map.keys().cloned().collect();
    for label in labels {
        let Some(state) = map.get(&label) else { continue };
        let Some(tab) = state.tabs.iter().find(|tab| tab.id == state.active) else {
            continue;
        };
        if window_is_alive(&tab.webview) {
            return Some(label);
        }
    }
    None
}

/// 取浏览器的激活标签（抓取流直接拿它来 eval/导航）。
// ── 广告拦截（管理员 11-23：喜欢 AdGuard，WebView2 装不了扩展，
// 就在 WebView2 的网络请求层做域名拦截——机制同 AdGuard 的网络过滤）──
static ADBLOCK_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_adblock_enabled(enabled: bool) {
    ADBLOCK_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn adblock_enabled() -> bool {
    ADBLOCK_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// 内置拦截名单：主流广告/弹窗/统计网络 + 漫画站常用广告源。
/// 想扩展：往 `%LOCALAPPDATA%\com.archivegallery.desktop\browser\adblock.txt`
/// 里一行一个域名追加（会在内置名单基础上生效）。
const ADBLOCK_DOMAINS: &[&str] = &[
    // 通用广告网络
    "doubleclick.net",
    "googlesyndication.com",
    "adservice.google.com",
    "adnxs.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "mgid.com",
    "revcontent.com",
    "adskeeper.co.uk",
    "adskeeper.com",
    " serving-sys.com",
    "smartadserver.com",
    // 成人站高频弹窗/跳转网络
    "exoclick.com",
    "exosrv.com",
    "exdynsrv.com",
    "exoclick.net",
    "popads.net",
    "popcash.net",
    "popmyads.com",
    "popunder.net",
    "propellerads.com",
    "propeller-tracking.com",
    "propellerclick.com",
    "hilltopads.net",
    "hilltopads.com",
    "adsco.re",
    "adsterratech.com",
    "revenuehits.com",
    "juicyads.com",
    "trafficjunky.net",
    "trafficjunky.com",
    "erosyndication.com",
    "clickadu.com",
    "onclickmega.com",
    "onclickperformance.com",
    "txtrk.net",
    "adcash.com",
    "adcashrevenue.com",
];

/// 读取用户自定义追加名单（可选文件，一行一个域名）。
fn custom_adblock_domains() -> Vec<String> {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let dir = std::env::var("LOCALAPPDATA")
                .map(|base| {
                    std::path::PathBuf::from(base)
                        .join("com.archivegallery.desktop")
                        .join("browser")
                        .join("adblock.txt")
                })
                .ok();
            let Some(path) = dir else { return Vec::new() };
            std::fs::read_to_string(path)
                .map(|content| {
                    content
                        .lines()
                        .map(|line| line.trim().to_lowercase())
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .collect()
                })
                .unwrap_or_default()
        })
        .clone()
}

fn adblock_host_blocked(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_lowercase();
    if host.is_empty() {
        return false;
    }
    let custom = custom_adblock_domains();
    ADBLOCK_DOMAINS
        .iter()
        .copied()
        .chain(custom.iter().map(|s| s.as_str()))
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// 给标签页挂上拦截回调：命中名单直接 404。
fn attach_adblock(
    builder: tauri::WebviewBuilder<tauri::Wry>,
) -> tauri::WebviewBuilder<tauri::Wry> {
    builder.on_web_resource_request(|request, response| {
        if !adblock_enabled() {
            return;
        }
        let Some(host) = request.uri().host() else {
            return;
        };
        if adblock_host_blocked(host) {
            *response.status_mut() = tauri::http::StatusCode::NOT_FOUND;
        }
    })
}

/// 拖拽排序：把 tabId 移到第 index 位（类似 Edge 的标签拖动，11-23）。
/// 只改 tabs 顺序（芯片行展示 + 前后切换语义）；webview 在窗口里的堆叠
/// 顺序与切换无关，不用动。
pub fn reorder_tab(label: &str, tab_id: &str, index: usize) -> Result<(), String> {
    with_state(label, |state| {
        let from = state
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or_else(|| "找不到要移动的标签".to_string())?;
        let to = index.min(state.tabs.len().saturating_sub(1));
        let tab = state.tabs.remove(from);
        state.tabs.insert(to, tab);
        Ok(())
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
}

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
    let state = guard.get_mut(label)?;
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
    open_hosted(app, url, label, opts, None)
}

/// 内置模式：宿主为已存在的窗口（P0 = main 窗）。webview 挂进宿主，
/// 尺寸/显隐由前端占位矩形驱动（d2_embed_sync / d2_embed_show）。
pub fn open_embedded(
    app: &AppHandle,
    url: Url,
    label: String,
    opts: BrowserOptions,
    host: Window,
) -> Result<String, String> {
    open_hosted(app, url, label, opts, Some(host))
}

fn open_hosted(
    app: &AppHandle,
    url: Url,
    label: String,
    opts: BrowserOptions,
    host: Option<Window>,
) -> Result<String, String> {
    if browser_exists(&label) {
        return Err("该浏览器窗口已在运行".to_string());
    }

    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_for_window = app.clone();
    let host_for_thread = host;
    let userscript_store = opts.userscript_store.clone();
    // 广告拦截开关跟随本次打开（设置里改完重开浏览窗口生效）。
    set_adblock_enabled(opts.adblock);
    let label_for_thread = label.clone();
    let first_tab_id = format!("{label}-tab-0");
    // 建窗整体放到独立线程（add_child 在同步命令/事件回调里调用会死锁，
    // tauri 文档明说；线程 + channel 是官方推荐姿势）。
    std::thread::spawn(move || {
        let label = label_for_thread;
        let run = || -> Result<(), String> {
            let embedded = host_for_thread.is_some();
            let window = match host_for_thread.clone() {
                Some(host) => {
                    crumb("内置模式：宿主已存在");
                    host
                }
                None => {
                    crumb(format!("建窗开始 {label}"));
                    tauri::WindowBuilder::new(&app_for_window, &label)
                        .title(opts.title.clone())
                        .inner_size(opts.size.0, opts.size.1)
                        .build()
                        .map_err(|error| format!("打开浏览窗口失败：{error}"))?
                }
            };
            crumb("窗口已建好，准备读尺寸");

            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size().unwrap_or_default();
            // 内置模式：React 马上会同步真实占位矩形，先按约定尺寸兜底摆位。
            let (width, height) = if embedded {
                (opts.size.0, opts.size.1)
            } else {
                (inner.width as f64 / scale, inner.height as f64 / scale)
            };

            // 工具栏：本仓公共页（真实占位，不遮内容；与引擎走队列契约）。
            crumb("建工具栏 webview");
            let toolbar = window
                .add_child(
                    WebviewBuilder::new(
                        format!("{label}-toolbar"),
                        // 内嵌模式：外壳工作台标签条接管多标签，内层标签条收掉。
                        WebviewUrl::App(if embedded {
                            "browser-toolbar.html?embedded=1".into()
                        } else {
                            "browser-toolbar.html".into()
                        }),
                    ),
                    LogicalPosition::new(0.0, 0.0),
                    LogicalSize::new(width, TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建工具栏失败：{error}"))?;
            crumb("工具栏就绪");

            let tab_url = url.to_string();
            let mut tab_builder = WebviewBuilder::new(
                first_tab_id.clone(),
                WebviewUrl::External(url),
            )
            .auto_resize();
            tab_builder = attach_tab_hooks(tab_builder, &app_for_window, &label, &first_tab_id);
            if let Some(store) = userscript_store.as_ref() {
                tab_builder = attach_userscripts(tab_builder, store, &label);
            }
            for script in &opts.extra_init_scripts {
                tab_builder = tab_builder.initialization_script(script.clone());
            }

            crumb(format!("建内容 webview {tab_url}"));
            let webview = window
                .add_child(
                    tab_builder,
                    LogicalPosition::new(0.0, TOOLBAR_HEIGHT),
                    LogicalSize::new(width, height - TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建内容标签失败：{error}"))?;
            crumb("内容 webview 就绪");

            let state = BrowserState {
                window: window.clone(),
                toolbar,
                tabs: vec![TabRecord {
                    id: first_tab_id.clone(),
                    webview,
                    url: tab_url,
                    title: String::new(),
                    zoom: 1.0,
                    loading: false,
                }],
                active: first_tab_id,
                // 设置以落盘文件为准：面板改过的主页/步进/快捷键，重开窗口即生效。
                // 没有文件时沿用宿主传进来的主页（D2 面板设置的那一个）。
                settings: load_settings_with_fallback(Some(opts.home.as_str())),
                settings_view: None,
                embedded,
                embed_rect: None,
                opts,
                last_pushed: String::new(),
            };
            crumb("注册浏览器状态");
            register_browser(&label, state, &app_for_window);
            crumb("注册完成，引擎已启动");
            // 快捷键开关按设置初始化（窗口刚建，标签脚本已经跑了默认值）。
            if let Ok(view) = active_webview(&label) {
                sync_shortcuts_flag(&label, &view);
            }
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
        .map(|guard| guard.contains_key(label))
        .unwrap_or(false)
}

/// 当前浏览器的激活内容 webview（抓取流直接拿它 eval/轮询）。
pub fn active_webview_of(label: &str) -> Result<Webview, String> {
    active_webview(label)
}

/// 激活标签导航到指定地址。
pub fn navigate_active(label: &str, url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|e| format!("网址格式不正确: {e}"))?;
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let webview = with_state(label, |state| {
        state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .map(|tab| tab.webview.clone())
    })
    .ok_or_else(|| "浏览器不存在".to_string())?
    .ok_or_else(|| "浏览器没有激活标签".to_string())?;
    webview
        .navigate(parsed)
        .map_err(|e| format!("导航失败: {e}"))
}

/// 后退/前进/刷新。
///
/// 刷新（2026-11-23 修）：原来走 `eval("location.reload()")`，页面正在导航/加载时
/// eval 会被丢弃，且失败只写日志、前端没有任何提示，表现就是「点了没反应」。
/// 改用 Webview 原生 `reload()`， Tauri 2 的 Webview 本身就支持；
/// 若是空白页（无 URL）则退回导航到主页，保证一定有可见反馈。
pub fn history_action(label: &str, action: &str) -> Result<(), String> {
    enum Plan {
        Eval(&'static str),
        Navigate(Url),
        Reload,
    }
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let (webview, plan) = with_state(label, |state| {
        let tab = state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .ok_or("浏览器没有激活标签")?;
        let plan = match action {
            "back" => Plan::Eval("history.back()"),
            "forward" => Plan::Eval("history.forward()"),
            "reload" => {
                if tab.url.trim().is_empty() || tab.url == "about:blank" {
                    // home 是 Url 类型：to_string() 后再判断，别当 String trim。
                    let home_str = state.opts.home.to_string();
                    let target = if home_str.trim().is_empty() || home_str == "about:blank" {
                        "https://e-hentai.org/"
                    } else {
                        home_str.as_str()
                    };
                    Plan::Navigate(Url::parse(target).map_err(|e| format!("主页地址无效: {e}"))?)
                } else {
                    // 原生 reload：不受页面导航状态/CSP 影响，失败能拿到 Err。
                    Plan::Reload
                }
            }
            _ => return Err("未知的历史动作".to_string()),
        };
        Ok((tab.webview.clone(), plan))
    })
    .ok_or_else(|| "浏览器不存在".to_string())??;
    match plan {
        Plan::Eval(script) => webview.eval(script),
        Plan::Navigate(url) => webview.navigate(url),
        Plan::Reload => webview.reload(),
    }
    .map_err(|e| format!("执行失败: {e}"))
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
            tab_builder = attach_tab_hooks(tab_builder, &app_for_tab, &label_for_tab, &tab_id_for_thread);
            if let Some(store) = store.as_ref() {
                tab_builder = attach_userscripts(tab_builder, store, &label_for_tab);
            }
            for script in &extra_init {
                tab_builder = tab_builder.initialization_script(script.clone());
            }
            crumb(format!("新标签建 webview {url_for_thread}"));
            let webview = window
                .add_child(
                    tab_builder,
                    LogicalPosition::new(0.0, TOOLBAR_HEIGHT),
                    LogicalSize::new(width, height - TOOLBAR_HEIGHT),
                )
                .map_err(|error| format!("创建标签失败：{error}"))?;
            crumb("新标签 webview 就绪");

            let tab_url = url_for_thread.to_string();
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
            let (to_hide, hide_self): (Vec<_>, Option<_>) = with_state(&label_for_tab, |state| {
                let record = TabRecord {
                    id: tab_id_for_thread.clone(),
                    webview,
                    url: tab_url,
                    title: String::new(),
                    zoom: 1.0,
                    loading: false,
                };
                if activate_for_thread {
                    let olds: Vec<_> = state.tabs.iter().map(|tab| tab.webview.clone()).collect();
                    state.tabs.push(record);
                    state.active = tab_id_for_thread.clone();
                    (olds, None)
                } else {
                    // 后台标签：先藏起来，激活的还是原来的
                    let own = record.webview.clone();
                    state.tabs.push(record);
                    (Vec::new(), Some(own))
                }
            })
            .ok_or("浏览器不存在")?;
            for old in to_hide {
                let _ = old.hide();
            }
            if let Some(own) = hide_self {
                let _ = own.hide();
            }
            // 新标签同样按当前设置初始化快捷键开关。
            if let Some(tab) = with_state(&label_for_tab, |state| {
                state
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id_for_thread)
                    .map(|tab| tab.webview.clone())
            })
            .flatten()
            {
                sync_shortcuts_flag(&label_for_tab, &tab);
            }
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
/// 打开 ⚙ 设置浮层（一条铺满窗口的 webview，页面自己画遮罩 + 居中卡片）。
///
/// 为什么不塞进工具栏 webview：工具栏只有 38px 高，任何弹层都会被裁掉。
/// 为什么不用新开原生窗口：设置是「当前这个浏览器」的上下文，浮层更贴，
/// 也不会多一个任务栏图标。webview 在内容标签之后创建，天然在最上层。
pub fn open_settings_window(app: &AppHandle, label: &str) -> Result<(), String> {
    // 已开着就聚焦（连点 ⚙ 不该开两个）。
    if let Some(view) = with_state(label, |state| state.settings_view.clone()).flatten() {
        let _ = view.show();
        let _ = view.set_focus();
        return Ok(());
    }
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_for_view = app.clone();
    let label_for_view = label.to_string();
    std::thread::spawn(move || {
        let run = || -> Result<(), String> {
            let (window, settings) = with_state(&label_for_view, |state| {
                (state.window.clone(), state.settings.clone())
            })
            .ok_or("浏览器不存在")?;
            let view_label = format!("{label_for_view}-settings");
            // 同 label 的残留（上次异常关闭）先清掉，否则 add_child 报重复。
            if let Some(old) = with_state(&label_for_view, |state| state.settings_view.clone()).flatten() {
                let _ = old.close();
                with_state(&label_for_view, |state| state.settings_view = None);
            }
            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size().unwrap_or_default();
            let width = inner.width as f64 / scale;
            let height = inner.height as f64 / scale;
            // 当前设置走 initialization_script 注入：add_child 返回后立刻 eval
            // 会打在 about:blank 上，页面一导航上下文就重置、值全丢。
            let payload = serde_json::to_string(&settings).unwrap_or_else(|_| "{}".to_string());
            let builder = WebviewBuilder::new(
                view_label,
                WebviewUrl::App("browser-settings.html".into()),
            )
            .auto_resize()
            .initialization_script(&format!("window.__xkSettings = {payload};"));
            let view = window
                .add_child(
                    builder,
                    LogicalPosition::new(0.0, 0.0),
                    LogicalSize::new(width, height),
                )
                .map_err(|error| format!("打开设置面板失败：{error}"))?;
            let _ = view.set_focus();
            with_state(&label_for_view, |state| state.settings_view = Some(view));
            start_settings_poller(&label_for_view, &app_for_view);
            Ok(())
        };
        let _ = tx.send(run());
    });
    match rx.recv_timeout(WINDOW_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err("打开设置面板超时".to_string()),
    }
}

/// 关掉设置浮层（页面点「取消」/点遮罩/保存完成后都走这里）。
pub fn close_settings_window(label: &str) -> Result<(), String> {
    let view = with_state(label, |state| state.settings_view.take()).flatten();
    if let Some(view) = view {
        let _ = view.close();
    }
    Ok(())
}

/// 设置浮层的请求轮询：保存 / 取消都从这里进来。
///
/// 单独一条线程而不是塞进工具栏轮询：设置页可以一直开着，工具栏轮询
/// 每 200ms 多 eval 一次会白烧；关掉浮层后这条线程自然退出。
fn start_settings_poller(label: &str, app: &AppHandle) {
    let label = label.to_string();
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(150));
            let Some(view) = with_state(&label, |state| state.settings_view.clone()).flatten()
            else {
                return;
            };
            let raw = match eval_json(
                &view,
                "JSON.stringify((function(){var q = window.__xkReqQ || []; window.__xkReqQ = []; return q;})())",
                Duration::from_secs(1),
            ) {
                Ok(raw) if !raw.trim().is_empty() && raw != "[]" => raw,
                // 读不到多半是页面已关/正在关，退出线程；关闭动作会把状态清掉。
                _ => {
                    if with_state(&label, |state| state.settings_view.is_none()).unwrap_or(true) {
                        return;
                    }
                    continue;
                }
            };
            let reqs: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
            for req in reqs {
                let action = req
                    .get("action")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                let arg = req
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                match action.as_str() {
                    "closeSettings" => {
                        let _ = close_settings_window(&label);
                        return;
                    }
                    _ => {
                        // config 复用工具栏那套：写运行态 + 落盘 + 同步快捷键开关。
                        if let Err(error) =
                            handle_toolbar_action(&app, &label, &view, &req, &action, &arg)
                        {
                            let _ = view.eval(&format!(
                                "window.__xkSetError && window.__xkSetError({});",
                                serde_json::to_string(&error)
                                    .unwrap_or_else(|_| "\"保存失败\"".to_string())
                            ));
                        } else if action == "config" {
                            let _ = view.eval("window.__xkSetSaved && window.__xkSetSaved();");
                        }
                    }
                }
            }
        }
    });
}

pub fn switch_tab(label: &str, tab_id: &str) -> Result<(), String> {
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let ops = with_state(label, |state| {
        state.active = tab_id.to_string();
        state
            .tabs
            .iter()
            .map(|tab| (tab.webview.clone(), tab.id == tab_id))
            .collect::<Vec<_>>()
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    for (webview, target) in ops {
        if target {
            let _ = webview.show();
            let _ = webview.set_focus();
        } else {
            let _ = webview.hide();
        }
    }
    push_state(label);
    Ok(())
}

/// 关闭标签；最后一个标签关闭时整个浏览器窗口一起关。
pub fn close_tab(label: &str, tab_id: &str) -> Result<(), String> {
    enum Plan {
        Missing,
        Tabs {
            close: Webview,
            show: Option<Webview>,
        },
        Window { close: Webview },
    }
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let plan = with_state(label, |state| {
        let Some(index) = state.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Plan::Missing;
        };
        let tab = state.tabs.remove(index);
        if state.tabs.is_empty() {
            return Plan::Window { close: tab.webview };
        }
        if state.active == tab_id {
            state.active = state
                .tabs
                .last()
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
        }
        let show = state
            .tabs
            .iter()
            .find(|tab| tab.id == state.active)
            .map(|tab| tab.webview.clone());
        Plan::Tabs {
            close: tab.webview,
            show,
        }
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;

    match plan {
        Plan::Missing => {}
        Plan::Window { close } => {
            let _ = close.close();
            close_browser(label)?;
        }
        Plan::Tabs { close, show } => {
            let _ = close.close();
            if let Some(webview) = show {
                let _ = webview.show();
            }
            push_state(label);
        }
    }
    Ok(())
}

/// 关闭整个浏览器窗口（状态在销毁事件里清理）。
pub fn close_browser(label: &str) -> Result<(), String> {
    // 内置模式：宿主是 main 窗，绝不能关窗——只拆 webview + 摘状态。
    if is_embedded(label) {
        return embed_close(label);
    }
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let window = with_state(label, |state| state.window.clone())
        .ok_or_else(|| "浏览器不存在".to_string())?;
    window
        .close()
        .map_err(|e| format!("关闭浏览器失败: {e}"))
}

/// 激活标签缩放（factor = 1.0 为 100%）。
pub fn set_zoom(label: &str, factor: f64) -> Result<(), String> {
    let factor = factor.clamp(0.25, 4.0);
    // 以前只调了 webview.set_zoom，忘了把值写回 TabRecord —— 重置 100% 后
    // 记录里还是旧缩放，再点一次 + 会从旧值上加（表现：缩放键越点越乱）。
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let (webview, record_err) = with_state(label, |state| {
        match state.tabs.iter_mut().find(|tab| tab.id == state.active) {
            Some(tab) => {
                tab.zoom = factor;
                (Some(tab.webview.clone()), None)
            }
            None => (None, Some("浏览器没有激活标签")),
        }
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    let result = match webview {
        Some(webview) => webview
            .set_zoom(factor)
            .map_err(|e| format!("缩放失败: {e}")),
        None => Err(record_err.unwrap_or_default().to_string()),
    };
    if result.is_ok() {
        push_state(label);
    }
    result
}

fn focus_tab_webview(tab_id: &str) -> Result<(), String> {
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let guard = BROWSERS.lock().map_err(|_| "浏览器状态异常".to_string())?;
    let map = &*guard;
    let target = map.values().find_map(|state| {
        state
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.webview.clone())
    });
    drop(guard);
    match target {
        Some(webview) => webview
            .set_focus()
            .map_err(|e| format!("聚焦失败: {e}")),
        None => Ok(()),
    }
}

/// 给内容标签挂钩子：弹窗→开新标签、下载→自动入库（带标签）、导航→记 URL。
fn attach_tab_hooks(
    builder: tauri::WebviewBuilder<tauri::Wry>,
    app: &AppHandle,
    label: &str,
    tab_id: &str,
) -> tauri::WebviewBuilder<tauri::Wry> {
    let builder = attach_adblock(builder);
    let label_for_new = label.to_string();
    let label_for_dl = label.to_string();
    let label_for_nav = label.to_string();
    let tab_id_for_nav = tab_id.to_string();
    let app_for_dl = AppHandle::clone(app);
    builder
        // Chromium 对「同一站点连续第二次自动下载」会弹权限框（"下载多个文件"），
        // 而且是个独立小窗，用户基本看不见 → 表现成「点了打包，库裡什么都没有」
        // （2026-11-24 实测复现：第一包进库、第二包被拦）。下载是我们自己在
        // on_download 里接管并入库的，允许即可；其它权限保持浏览器默认。
        .on_permission_request(|_webview, kind| {
            crumb(format!("权限请求 {kind:?}"));
            match kind {
                tauri::webview::PermissionKind::AutomaticDownloads => {
                    tauri::webview::PermissionResponse::Allow
                }
                _ => tauri::webview::PermissionResponse::Default,
            }
        })
        // 弹窗接管 v2：新窗一律开成新标签（后台建，建好即激活）。
        .on_new_window(move |new_url, _features| {
            queue_pending_tab(&label_for_new, &new_url.to_string(), true);
            NewWindowResponse::Deny
        })
        .on_download({
            let label_for_download = label_for_dl.clone();
            let app_for_download = app_for_dl.clone();
            move |webview, event| {
                match event {
                    DownloadEvent::Requested { url, destination } => {
                        // 两条去向：
                        // - 压缩包（以及认不出扩展名的，可能是 blob 打包）→ 画匣缓存，
                        //   下完自动入库 + 挂标签（管理员 2026-11-24 要求）；
                        // - 图片/视频/文档（右键「打开原图并另存」走这条）→
                        //   用户的 `图片\画匣下载`。以前一律丢缓存再当图集导入，
                        //   结果是「入库失败：只支持 zip / cbz」的假报警（2026-11-24 修）。
                        //
                        // 文件名要留住 WebView2 给的 suggested name（来自页面的
                        // `<a download="标题.zip">`）：以前一律用 URL 末段推导，
                        // 织机打包走 blob: 下载，末段是随机 uuid、没有扩展名，
                        // 入库那一步直接被「不是 zip」拒掉。
                        let suggested = archive_name_from(destination);
                        let pictures = default_download_dir();
                        let to_pictures = suggested.is_none() && is_media_download(destination);                        let target_dir = if to_pictures {
                            Some(pictures.clone())
                        } else {
                            app_for_download
                                .path()
                                .app_cache_dir()
                                .map(|cache| cache.join("d2").join("downloads"))
                                .ok()
                        };
                        if let Some(dir) = target_dir {
                            let _ = std::fs::create_dir_all(&dir);
                            *destination = dir.join(safe_download_name(destination, &url));
                        }
                        log_for(
                            &label_for_download,
                            &format!("下载开始：{url} → {}", destination.display()),
                        );
                        // 只给压缩包抓元数据：图片另存不需要标签，也省一次 eval。
                        if !destination.starts_with(&pictures) {
                            let key = destination.to_string_lossy().to_string();
                            let view = webview.clone();
                            std::thread::spawn(move || {
                                if let Some(meta) = read_page_meta(&view) {
                                    if let Ok(mut map) = DOWNLOAD_META.lock() {
                                        // 清掉早就没下文的旧条目，别让表无限长。
                                        if map.len() > 24 {
                                            map.clear();
                                        }
                                        map.insert(key, meta);
                                    }
                                }
                            });
                        }
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
                        if success {
                            if let Some(file) = path {
                                if file.starts_with(default_download_dir()) {
                                    // 图片/视频另存：不导入，告诉用户去哪找。
                                    log_for(
                                        &label_for_download,
                                        &format!("已另存：{}", file.display()),
                                    );
                                    toast_toolbar(&label_for_download, "已另存到 图片\\画匣下载", "ok");
                                } else {
                                    let app_import = app_for_download.clone();
                                    let label_import = label_for_download.clone();
                                    let cached = file.clone();
                                    // 导入是阻塞的磁盘/数据库操作，别卡住 webview 线程。
                                    std::thread::spawn(move || {
                                        let meta = take_download_meta(&cached);
                                        let outcome = crate::d2::import_downloaded_meta(
                                            &app_import,
                                            cached.clone(),
                                            &meta.title,
                                            &meta.tags,
                                        );
                                        match outcome {
                                            Ok(message) => {
                                                log_for(&label_import, &format!("自动入库：{message}"));
                                                toast_toolbar(&label_import, &format!("✅ 已入库：{message}（不用再下一次）"), "ok");
                                                // 导入是「复制临时文件再改名」，源文件还留在
                                                // d2/downloads 里。库在别的盘时这就是每本多一份
                                                // 副本（管理员库在 F:，缓存会在 C: 越堆越大）。
                                                // 入库成功就把缓存清掉；移动型导入时文件已不
                                                // 存在，remove 失败也无所谓。
                                                if let Ok(dir) = app_import.path().app_cache_dir() {
                                                    let downloads = dir.join("d2").join("downloads");
                                                    if cached.starts_with(downloads) {
                                                        let _ = std::fs::remove_file(&cached);
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                log_for(&label_import, &format!("自动入库失败：{error}"));
                                                toast_toolbar(
                                                    &label_import,
                                                    &format!("入库失败：{error}"),
                                                    "bad",
                                                );
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }
                    // DownloadEvent 标记 #[non_exhaustive]
                    _ => {}
                }
                true // 放行下载（目的地已改到画匣缓存）
            }
        })
        .on_navigation({
            let label_for_navigation = label_for_nav.clone();
            let tab_id_for_navigation = tab_id_for_nav.clone();
            move |url| {
                crumb(format!("导航 {url}"));
                note_tab_url(&label_for_navigation, &tab_id_for_navigation, &url.to_string());
                // 导航一发生就先进「加载中」，工具栏刷新键立刻变成停止键。
                note_tab_loading(&label_for_navigation, &tab_id_for_navigation, true);
                true
            }
        })
        // 页面标题（之前没接，标签芯片只能显示 URL）。
        .on_document_title_changed({
            let label_for_title = label_for_nav.clone();
            let tab_id_for_title = tab_id_for_nav.clone();
            move |_webview, title| {
                note_tab_title(&label_for_title, &tab_id_for_title, &title);
            }
        })
        // 加载完成（含失败）：把「加载中」清掉，进度指示收尾。
        .on_page_load({
            let label_for_load = label_for_nav.clone();
            let tab_id_for_load = tab_id_for_nav.clone();
            move |webview, payload| {
                crumb(format!("加载事件 {:?}", payload.event()));
                let loading = payload.event() == tauri::webview::PageLoadEvent::Started;
                note_tab_loading(&label_for_load, &tab_id_for_load, loading);
                // 只在「加载完成」后写快捷键开关：新文档上下文此时才稳定，
                // 写入的 __xkShortcuts 不会被随后的导航重置掉。
                if !loading {
                    sync_shortcuts_flag(&label_for_load, &webview);
                }
            }
        })
        .initialization_script(CONTENT_SHORTCUTS_SCRIPT)
        // 右键菜单（引擎级）：页面 preventDefault 掉 WebView2 原生菜单后，
        // 由本脚本画匣系自己的菜单。
        .initialization_script(CONTEXT_MENU_SCRIPT)
}

/// 下载瞬间抓到的作品页元数据（标题 + tag 分组）。
///
/// 只从页面拿，不经网络：`__d2Meta()` 是宿主注入的站点解析器（e-hentai /
/// nhentai / 18comic / wnacg 各一套），拿不到就返回 None，导入退化为普通入库。
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct PageMeta {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: HashMap<String, Vec<String>>,
}

impl PageMeta {
    fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.tags.is_empty()
    }
}

/// 下载目标路径 → 元数据。下载开始与结束是两条 WebView2 事件，中间隔着整个
/// 传输过程，只能借这张表接力。
static DOWNLOAD_META: std::sync::LazyLock<Mutex<HashMap<String, PageMeta>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 图片/视频/文档类下载（右键「打开原图并另存」、页面直链下载走这条）。
///
/// 这类文件不该进图集库，也不该报「只支持 zip / cbz」——落到
/// `图片\画匣下载` 就好。
fn is_media_download(destination: &std::path::Path) -> bool {
    let ext = destination
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "avif"
            | "bmp"
            | "svg"
            | "mp4"
            | "webm"
            | "mkv"
            | "mp3"
            | "m4a"
            | "wav"
            | "pdf"
            | "txt"
            | "csv"
            | "json"
            | "html"
            | "htm"
    )
}

/// 非图集下载的落点：`%USERPROFILE%\Pictures\画匣下载`（与 gallery_dl 一致）。
fn default_download_dir() -> std::path::PathBuf {
    match std::env::var("USERPROFILE") {
        Ok(profile) => std::path::PathBuf::from(profile)
            .join("Pictures")
            .join("画匣下载"),
        Err(_) => std::env::temp_dir().join("画匣下载"),
    }
}

/// WebView2 给的「建议文件名」→ 可落地的压缩包名。
///
/// 页面 `<a download="…">` 里的名字常常带 Windows 非法字符（e-hentai 标题里
/// 的 `?` `:` `|` 很常见），直接塞进 `SetResultFilePath` 会让下载直接失败，
/// 所以统一过一遍 d2 的文件名清洗，只保留压缩包类扩展名。
fn archive_name_from(destination: &std::path::Path) -> Option<String> {
    let name = destination.file_name()?.to_string_lossy().to_string();
    let path = std::path::Path::new(&name);
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "zip" | "cbz" | "rar" | "7z") {
        return None;
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("下载");
    let cleaned = crate::d2::sanitize_file_name(stem);
    Some(format!("{cleaned}.{ext}"))
}

/// 最终落盘文件名：优先保留页面给的名字（只清非法字符），实在没有才用 URL 推导。
///
/// 图片另存以前走到 `download_file_name(url)`：blob URL 末段是 uuid、没有扩展名，
/// 存出来叫 `download.bin`，用户双击都打不开（2026-11-24 修）。
fn safe_download_name(destination: &std::path::Path, url: &Url) -> String {
    if let Some(name) = archive_name_from(destination) {
        return name;
    }
    if let Some(name) = destination.file_name().map(|name| name.to_string_lossy().to_string()) {
        let path = std::path::Path::new(&name);
        let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|value| value.to_str()).unwrap_or("");
        if !stem.is_empty() && !ext.is_empty() {
            let cleaned = crate::d2::sanitize_file_name(stem);
            return format!("{cleaned}.{ext}");
        }
    }
    download_file_name(url)
}

/// 读当前页面的作品元数据（最多等 3 秒，页面忙也不拖死下载）。
fn read_page_meta(webview: &Webview) -> Option<PageMeta> {
    let raw = eval_json(
        webview,
        "JSON.stringify((function(){ \
           try { \
             if (typeof window.__d2Meta === 'function') { var m = window.__d2Meta(); \
               if (m && (m.title || (m.tags && Object.keys(m.tags).length))) return m; } \
             return { title: document.title || '', tags: {} }; \
           } catch (e) { return { title: '', tags: {}, error: String(e) }; } \
         })())",
        Duration::from_secs(3),
    )
    .ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let meta: PageMeta = serde_json::from_str(&raw).ok()?;
    if meta.is_empty() {
        None
    } else {
        Some(meta)
    }
}

/// 取（并移除）某次下载的元数据。下载可能比抓取快，所以给一小段等待窗口；
/// 路径分隔符/规范化的细微差异按文件名再兜一层。
fn take_download_meta(path: &std::path::Path) -> PageMeta {
    let key = path.to_string_lossy().to_string();
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    for _ in 0..25 {
        if let Ok(mut map) = DOWNLOAD_META.lock() {
            if let Some(meta) = map.remove(&key) {
                return meta;
            }
            if !name.is_empty() {
                let matched = map
                    .keys()
                    .find(|candidate| candidate.ends_with(&name))
                    .cloned();
                if let Some(found) = matched {
                    if let Some(meta) = map.remove(&found) {
                        return meta;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    PageMeta::default()
}

/// 往工具栏弹一句提示（kind: ok / bad）。工具栏是唯一始终可见的窗格。
fn toast_toolbar(label: &str, text: &str, kind: &str) {
    let Some(toolbar) = toolbar_of(label) else {
        return;
    };
    let message = serde_json::to_string(text).unwrap_or_else(|_| "\"完成\"".to_string());
    let kind = serde_json::to_string(kind).unwrap_or_else(|_| "\"bad\"".to_string());
    let _ = toolbar.eval(&format!(
        "window.__xkToast && window.__xkToast({message}, {kind});"
    ));
}

/// 从下载 URL 提取安全文件名（去掉查询串与路径穿越字符）。
fn download_file_name(url: &Url) -> String {
    let raw = url
        .path_segments()
        .and_then(|segments| segments.filter(|part| !part.is_empty()).last())
        .unwrap_or("download.bin");
    let decoded = raw.replace("%20", " ");
    let cleaned: String = decoded
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') || !ch.is_ascii() {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || !cleaned.contains('.') {
        format!("download-{}.bin", crate::fsutil::now_ms())
    } else {
        cleaned.to_string()
    }
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


/// 内容页快捷键脚本（引擎级，所有内容标签注入）。
///
/// 内嵌 WebView2 里，焦点在网页上时工具栏收不到键盘事件，所以快捷键必须
/// 在内容页捕获、写成 `__xkReqQ` 交给引擎执行。`window.__xkShortcuts` 是
/// 开关（设置面板可关，避免和站点自己的 Ctrl+W 之类冲突）。
const CONTENT_SHORTCUTS_SCRIPT: &str = r###"
(function () {
  if (window.__xkKeys) return;
  window.__xkKeys = true;
  // 默认开；引擎在每次「加载完成」和「保存设置」后写入真实开关值。
  if (window.__xkShortcuts === undefined) window.__xkShortcuts = true;
  function push(action, extra) {
    var item = { action: action };
    if (extra) for (var k in extra) item[k] = extra[k];
    (window.__xkReqQ = window.__xkReqQ || []).push(item);
  }
  document.addEventListener("keydown", function (e) {
    if (window.__xkShortcuts === false) return;
    var ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.altKey && !e.shiftKey) {
      var k = (e.key || "").toLowerCase();
      if (k === "t") { e.preventDefault(); push("new"); return; }
      if (k === "w") { e.preventDefault(); push("close"); return; }
      if (k === "r") { e.preventDefault(); push("reload"); return; }
      if (k === "l") { e.preventDefault(); push("focusAddr"); return; }
      if (k === "=" || k === "+") { e.preventDefault(); push("zoomIn"); return; }
      if (k === "-") { e.preventDefault(); push("zoomOut"); return; }
      if (k === "0") { e.preventDefault(); push("zoomReset"); return; }
    }
    if (e.altKey && !e.ctrlKey && e.key === "ArrowLeft") {
      e.preventDefault(); push("back"); return;
    }
    if (e.altKey && !e.ctrlKey && e.key === "ArrowRight") {
      e.preventDefault(); push("forward"); return;
    }
    if (e.key === "F5") { e.preventDefault(); push("reload"); return; }
    if (e.ctrlKey && e.key === "Tab") {
      // Ctrl+Tab 在页面里没意义，留给引擎切下一个标签。
      e.preventDefault();
      push("nextTab");
    }
  }, true);
})();
"###;

/// 内容页右键菜单脚本（引擎级，所有内容标签注入）。
///
/// WebView2 的规则：页面在 `contextmenu` 事件里 `preventDefault()`，原生默认
/// 菜单就不弹——所以纯页内实现即可。菜单本体也是 Shadow DOM（不沾站点样式）。
/// 本地动作（复制/后退/刷新）页面内完成；需要引擎的动作（开新标签）推进
/// `window.__xkReqQ` 队列，由引擎轮询器消化（与工具栏同一套契约，数组队列
/// 保证连点两次不会互相覆盖）。
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
      items.push(item("在新标签打开链接", function () { (window.__xkReqQ = window.__xkReqQ || []).push({ action: "new", url: link.href }); }));
      items.push(item("复制链接地址", function () { copyText(link.href); }));
    }
    if (img && (img.currentSrc || img.src)) {
      items.push(item("在新标签打开图片", function () { (window.__xkReqQ = window.__xkReqQ || []).push({ action: "new", url: img.currentSrc || img.src }); }));
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
            if let Ok(mut map) = BROWSERS.lock() {
                map.remove(&label_for_close);
            }
        }
    });
    // 注册失败直接炸出来（以前 insert 静默丢失，整台引擎哑掉还查不出原因）。
    match BROWSERS.lock() {
        Ok(mut map) => {
            map.insert(label.to_string(), state);
        }
        Err(error) => {
            eprintln!("[browser] 注册浏览器失败（状态表中毒）：{error}");
            return;
        }
    }
    // 重算布局（窗口尺寸可能在建标签期间变了）。
    reposition(label);
    // 引擎轮询器：工具栏请求 + 待建标签消化 + 油猴跨域 XHR。
    start_toolbar_poller(label, app.clone());
    start_pending_tab_worker(app, label, Arc::new(AtomicBool::new(false)));
    start_gm_poller(label, app.clone());
}

/// 工具栏请求队列轮询器：读 `__xkReqQ` 执行动作，把最新状态写回 `__xkState`。
///
/// 用**数组队列**而不是单槽：单槽下 200ms 内连点两次（很常见：连点两下放大、
/// 快速切标签再刷新）第二次会直接覆盖第一次，表现就是「点了没反应」。
fn start_toolbar_poller(label: &str, app: AppHandle) {
    let label = label.to_string();
    std::thread::spawn(move || {
        // 留一行启动日志：引擎活着还是哑掉，事后能一眼看出来。
        log_for(&label, "工具栏轮询器已启动（200ms/轮）");
        loop {
            std::thread::sleep(TOOLBAR_POLL_INTERVAL);
            if !browser_exists(&label) {
                log_for(&label, "工具栏轮询器退场（浏览器已关闭）");
                return;
            }
            // 工具栏 webview 偶尔会在重建/隐藏时短暂取不到；原来直接 return
            // 会让轮询线程永久退出（刷新/地址栏从此全失效），改为跳过本轮。
            let Some(toolbar) = toolbar_of(&label) else {
                continue;
            };
            // 读取与清空放在同一次 eval 里完成（原子取走），避免执行期间新点的
            // 动作被误清，也避免同一批动作重复执行。
            let raw = match eval_json(
                &toolbar,
                "JSON.stringify((function(){var q = window.__xkReqQ || []; window.__xkReqQ = []; return q;})())",
                Duration::from_secs(2),
            ) {
                Ok(raw) if !raw.trim().is_empty() && raw != "[]" => raw,
                _ => {
                    push_state(&label);
                    continue;
                }
            };
            let reqs: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
                Ok(value) => value,
                Err(_) => continue,
            };
            for req in reqs {
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
                let result = handle_toolbar_action(&app, &label, &toolbar, &req, &action, &arg);
                if let Err(error) = result {
                    log_for(&label, &format!("工具栏动作 {action} 失败：{error}"));
                    // 失败要让用户看见，不能只在日志里安静地躺着。
                    let _ = toolbar.eval(&format!(
                        "window.__xkToast && window.__xkToast({});",
                        serde_json::to_string(&format!("{action} 失败：{error}"))
                            .unwrap_or_else(|_| "\"操作失败\"".to_string())
                    ));
                }
            }
            push_state(&label);
        }
    });
}

/// 单个工具栏动作的执行体（拆出来是为了 poller 循环短、错误处理统一）。
fn handle_toolbar_action(
    app: &AppHandle,
    label: &str,
    toolbar: &Webview,
    req: &serde_json::Value,
    action: &str,
    arg: &str,
) -> Result<(), String> {
    match action {
        // 工具栏页 JS 就绪后主动握手：无条件推一次全量状态。
        // 原来的去重（last_pushed）会在首次 eval 早于页面就绪时把
        // 那次状态记为"已推"，之后相同 JSON 不再推送 → 地址栏永久空白。
        "__pull" => {
            with_state(label, |state| state.last_pushed.clear());
            push_state(label);
            Ok(())
        }
        "nav" if !arg.is_empty() => {
            let target = resolve_address(label, arg);
            navigate_active(label, &target)
        }
        "back" => history_action(label, "back"),
        "forward" => history_action(label, "forward"),
        "reload" => history_action(label, "reload"),
        "stop" => stop_loading(label),
        "reorder" => {
            let tab_id = req
                .get("tabId")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let index = req
                .get("index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize;
            reorder_tab(label, &tab_id, index)
        }
        // 工具栏「+」：url 为空时按设置决定开主页还是空白页。
        "new" => {
            let url = if arg.trim().is_empty() {
                with_state(label, |state| match state.settings.new_tab.as_str() {
                    "blank" => "about:blank".to_string(),
                    _ => {
                        let home = state.settings.home.clone();
                        // 主页是画匣新页时 webview 开不了，退回空白页。
                        if home.trim().starts_with(HUAXIA_NEWTAB) {
                            "about:blank".to_string()
                        } else {
                            home
                        }
                    }
                })
                .unwrap_or_else(|| "about:blank".to_string())
            } else {
                arg.to_string()
            };
            new_tab(app, label, &url, true).map(|_| ())
        }
        "home" => {
            let home = with_state(label, |state| state.settings.home.clone())
                .unwrap_or_default();
            if home.trim().starts_with(HUAXIA_NEWTAB) {
                // 画匣新页由主窗渲染：通知它把本标签切回新页，webview 随卸载退出。
                let _ = tauri::Emitter::emit(app, "huaxia://newtab", label.to_string());
                Ok(())
            } else if home.trim().is_empty() {
                Err("主页还没设置（点 ⚙ 配一个）".to_string())
            } else {
                navigate_active(label, &home)
            }
        }
        "close" => {
            if arg.is_empty() {
                // 没有指定标签时关当前激活标签，符合「点 × 」的直觉。
                let active = with_state(label, |state| state.active.clone()).unwrap_or_default();
                close_tab(label, &active)
            } else {
                close_tab(label, arg)
            }
        }
        "switch" => switch_tab(label, arg),
        // Ctrl+Tab / Ctrl+Shift+Tab：在所有标签间循环。
        "nextTab" => cycle_tab(label, 1),
        "prevTab" => cycle_tab(label, -1),
        "zoomIn" => adjust_zoom(label, zoom_step(label)),
        "zoomOut" => adjust_zoom(label, -zoom_step(label)),
        "zoomReset" => set_zoom(label, 1.0),
        // Ctrl+L / 地址栏快捷键：把焦点交给工具栏里的地址栏。
        "focusAddr" => {
            let _ = toolbar.set_focus();
            let _ = toolbar.eval(
                "window.__xkFocusAddr = true; setTimeout(function(){ window.__xkFocusAddr = false; }, 800);",
            );
            Ok(())
        }
        // 工具栏 ⚙：打开设置浮层。
        "openSettings" => open_settings_window(app, label),
        // ⚙ 设置面板保存：写运行态 + 落盘，立即生效（主页/步进/快捷键不用重开）。
        "config" => {
            let incoming = req.get("config").cloned().unwrap_or(serde_json::Value::Null);
            let parsed: BrowserSettings = serde_json::from_value(incoming)
                .map_err(|error| format!("设置格式不对：{error}"))?;
            let settings = parsed.sanitized();
            save_settings(&settings)?;
            let shortcuts = settings.shortcuts;
            with_state(label, |state| {
                state.settings = settings.clone();
                state.opts.home = Url::parse(&settings.home)
                    .unwrap_or_else(|_| state.opts.home.clone());
                state.last_pushed.clear();
            });
            // 快捷键开关要立刻作用到已经打开的标签（它们早就注入完脚本了）。
            if let Some(views) = tab_webviews(label) {
                for view in views {
                    let _ = view.eval(&format!("window.__xkShortcuts = {shortcuts};"));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 缩放步进（设置面板可调，非法值退回 0.1）。
fn zoom_step(label: &str) -> f64 {
    with_state(label, |state| state.settings.zoom_step)
        .unwrap_or(0.1)
        .clamp(0.05, 0.5)
}

/// 地址栏输入 → 真正要导航的 URL。
///
/// 看起来不像网址（没有点、带空格、不是 localhost）就走搜索模板：
/// 以前直接 `https://{arg}`，用户输入「xxx 漫画」会得到一个坏网址，表现就是
/// 「地址栏按回车没反应」。
fn resolve_address(label: &str, raw: &str) -> String {
    let template = with_state(label, |state| state.settings.clone()).unwrap_or_default();
    address_target(raw, &template)
}

/// `resolve_address` 的纯函数部分（单测直接测它，不必造浏览器状态）。
fn address_target(raw: &str, settings: &BrowserSettings) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("about:")
        || trimmed.starts_with("file:")
    {
        return trimmed.to_string();
    }
    // 像域名的判据：第一段（到第一个 / 之前）带点、整串没空格、不是以点
    // 开头/结尾。这样粘贴「e-hentai.org/g/1/2/」这种没写 scheme 的作品
    // 地址也能直接去，而不是被当成搜索词。
    let host_part = trimmed.split('/').next().unwrap_or("");
    let host_ok = !trimmed.contains(' ')
        && !host_part.starts_with('.')
        && !host_part.ends_with('.')
        && (host_part.contains('.')
            || host_part == "localhost"
            || host_part.starts_with("localhost:"));
    if host_ok {
        return format!("https://{trimmed}");
    }
    settings.search_url_for(trimmed)
}

fn adjust_zoom(label: &str, delta: f64) -> Result<(), String> {
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let (webview, record_err) = with_state(label, |state| {
        match state.tabs.iter_mut().find(|tab| tab.id == state.active) {
            Some(tab) => {
                let zoom = (tab.zoom + delta).clamp(0.25, 4.0);
                tab.zoom = zoom;
                (Some((tab.webview.clone(), zoom)), None)
            }
            None => (None, Some("浏览器没有激活标签")),
        }
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    let result = match webview {
        Some((webview, factor)) => webview
            .set_zoom(factor)
            .map_err(|e| format!("缩放失败: {e}")),
        None => Err(record_err.unwrap_or_default().to_string()),
    };
    // 缩放值现在进状态：不推的话工具栏那个百分比永远停在旧值。
    if result.is_ok() {
        push_state(label);
    }
    result
}

/// 停止加载（刷新键在加载中时的第二种形态）。
///
/// Tauri 2 的 Webview 没有暴露 Stop，页面侧 `window.stop()` 效果等价
/// （中止后续资源请求，已渲染内容保留）。
fn stop_loading(label: &str) -> Result<(), String> {
    let webview = active_webview(label)?;
    webview
        .eval("try { window.stop(); } catch (e) {}")
        .map_err(|error| format!("停止加载失败：{error}"))?;
    let active = with_state(label, |state| state.active.clone()).unwrap_or_default();
    if !active.is_empty() {
        note_tab_loading(label, &active, false);
    }
    Ok(())
}

/// 在标签间循环切换（Ctrl+Tab 语义；到头回绕）。
fn cycle_tab(label: &str, delta: i32) -> Result<(), String> {
    let target = with_state(label, |state| {
        if state.tabs.len() < 2 {
            return String::new();
        }
        let current = state
            .tabs
            .iter()
            .position(|tab| tab.id == state.active)
            .unwrap_or(0) as i32;
        let len = state.tabs.len() as i32;
        let next = ((current + delta) % len + len) % len;
        state.tabs[next as usize].id.clone()
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    if target.is_empty() {
        return Ok(());
    }
    switch_tab(label, &target)
}

/// 把「快捷键开关」同步给某条内容页（设置改了要立刻作用于已开标签）。
fn sync_shortcuts_flag(label: &str, webview: &Webview) {
    let on = with_state(label, |state| state.settings.shortcuts).unwrap_or(true);
    let _ = webview.eval(&format!("window.__xkShortcuts = {on};"));
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

/// 内容页请求（右键菜单「在新标签打开」等）：与工具栏同一套队列契约。
fn poll_tab_req(label: &str, webview: &Webview) {
    let raw = match eval_json(
        webview,
        "JSON.stringify((function(){var q = window.__xkReqQ || []; window.__xkReqQ = []; return q;})())",
        Duration::from_secs(2),
    ) {
        Ok(raw) if !raw.trim().is_empty() && raw != "[]" => raw,
        _ => return,
    };
    let reqs: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return,
    };
    for req in reqs {
        if req.get("action").and_then(|value| value.as_str()) != Some("new") {
            continue;
        }
        let Some(url) = req.get("url").and_then(|value| value.as_str()) else {
            continue;
        };
        if url.is_empty() {
            continue;
        }
        let Some(app) = app_of(label) else {
            return;
        };
        if let Err(error) = new_tab(&app, label, url, true) {
            log_for(label, &format!("右键新标签失败：{error}"));
        }
    }
}

/// 油猴跨域 XHR：轮询页面侧 `window.__xkGmXhr` 队列，把 pending 的请求用
/// curl 发出去，结果写回队列，页面侧垫片轮到自己时 resolve。
/// 页面始终拿不到 IPC，跨域能力由宿主进程代劳——安全边界不变。
/// 页内「认领」脚本：把所有 pending 条目原子地置成 running 并原样带回。
///
/// 取内容与改状态必须在同一次 eval 里完成。以前是「读一遍 → Rust 判 pending
/// → 阻塞发请求 → 写回」，而轮询间隔只有 250ms：一个慢请求（图片动辄几秒）
/// 就能让同一批里的其余条目被反复重发，也会把这一整条轮询线程（连工具栏
/// 请求一起）堵住。现在认领即置忙，真正的 curl 放到独立线程并发跑。
const GM_CLAIM_SCRIPT: &str = "JSON.stringify((function(){\
var q = window.__xkGmXhr, out = {}; if (!q || !q.items) return out;\
for (var k in q.items) { var it = q.items[k];\
if (it && it.state === 'pending' && !it.aborted) { it.state = 'running'; out[k] = it; } }\
return out; })())";

/// 同时在飞的 GM 跨域请求上限。漫画织机的批量下载会一次抛十几个，
/// 全放出去会互相抢带宽还容易触发站点限流；超出的留在 pending 下一轮再取。
const GM_XHR_MAX_INFLIGHT: usize = 6;
static GM_XHR_INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static GM_XHR_SEQ: AtomicU64 = AtomicU64::new(0);

fn poll_gm_xhr_queue(webview: &Webview) {
    if !window_is_alive(webview) {
        return;
    }
    if GM_XHR_INFLIGHT.load(Ordering::Relaxed) >= GM_XHR_MAX_INFLIGHT {
        return;
    }
    let raw = match eval_json(webview, GM_CLAIM_SCRIPT, Duration::from_secs(2)) {
        Ok(raw) if !raw.trim().is_empty() && raw != "{}" => raw,
        _ => return,
    };
    let items: HashMap<String, serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(items) => items,
        Err(_) => return,
    };

    for (id, item) in items {
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
        // blob / arraybuffer 说明脚本要的是字节，不是文本 —— 走 base64 回传。
        let response_type = item
            .get("responseType")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let binary = matches!(response_type.as_str(), "blob" | "arraybuffer");
        // 脚本挂了 onprogress 才开进度探针——没挂就别每 150ms 白 eval 一次。
        let has_progress = item
            .get("hasProgress")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(map) = item.get("headers").and_then(|value| value.as_object()) {
            for (key, value) in map {
                if let Some(value) = value.as_str() {
                    headers.push((key.clone(), value.to_string()));
                }
            }
        }

        GM_XHR_INFLIGHT.fetch_add(1, Ordering::Relaxed);
        let webview = webview.clone();
        std::thread::spawn(move || {
            let mut probe = has_progress.then(|| ProgressProbe::new(&webview, &id));
            // 取消探针总开：文本请求也可能是大 JSON/慢接口，点了取消同样该停。
            let mut canceller = CancelProbe::new(&webview, &id);
            let outcome = curl_xhr(
                &url,
                &method,
                &headers,
                &data,
                binary,
                probe.as_mut().map(|probe| probe as &mut dyn ProgressSink),
                Some(&mut canceller as &mut dyn Canceller),
            );
            let result = serde_json::json!({
                "state": "done",
                "status": outcome.status,
                "responseText": outcome.text,
                "responseB64": outcome.base64,
                "responseHeaders": outcome.headers,
                "error": outcome.error,
                "errorText": outcome.error_text,
                "aborted": outcome.aborted,
            })
            .to_string();
            let script = format!(
                "(function(){{ var q = window.__xkGmXhr && window.__xkGmXhr.items['{id}']; \
                 if (!q) return; var r = {result}; \
                 q.state = r.state; q.status = r.status; q.responseText = r.responseText; \
                 q.responseB64 = r.responseB64; q.responseHeaders = r.responseHeaders; \
                 q.error = r.error; q.errorText = r.errorText; q.aborted = r.aborted; }})();",
                id = id.replace('\\', "\\\\").replace('\'', "\\'"),
                result = result.replace('\'', "\\'"),
            );
            let _ = webview.eval(&script);
            GM_XHR_INFLIGHT.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// 脚本没显式指定 UA 时补的默认值。图床/图库普遍按 UA 拦 curl 的裸请求，
/// 不带 UA 会直接 403，脚本那边只会看到「下载失败」而不知为何。
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";

/// 二进制回传上限。再大就把单个响应序列化成几十 MB 的 JS 字面量去 eval，
/// 页面会明显卡顿；正常漫画单页远低于这个尺寸。
const GM_XHR_MAX_BINARY: usize = 48 * 1024 * 1024;

/// GM 跨域请求的结果。
///
/// `text` 与 `base64` 二选一：脚本要 `blob`/`arraybuffer` 时只填 base64。
/// 以前不管 `responseType` 一律按 UTF-8 文本回，漫画织机取图片就拿到一串
/// U+FFFD，画出来是空白、批量下载必然失败（2026-11-23 补）。
struct XhrOutcome {
    status: u16,
    text: String,
    base64: String,
    headers: String,
    error: bool,
    /// 传输失败时的说明（含 URL）。漫画织机的 `onerror` 里会直接拿它做
    /// `response.error.match(/https?:\/\/.../)` 提域名报错文案——留空或
    /// undefined 会让它自己的回调抛异常、Promise 永不 settle，界面永远
    /// 转圈（2026-11-23 补）。
    error_text: String,
    /// 被页面 `abort()` 取消（curl 子进程已 kill，不是「网络失败」）。
    aborted: bool,
}

/// 取消探针：curl 跑的过程中问一句「页面还要不要结果」。
///
/// 以前 `abort()` 只是页面自己停掉定时器，curl 那头照样把整个文件下完——
/// 用户点了取消，磁盘和带宽还在被吃（织机一次十几张图尤其明显）。
trait Canceller {
    /// 返回 true 表示页面已经放弃这个请求，宿主应立刻杀子进程。
    fn cancelled(&mut self) -> bool;
}

/// 靠 eval 读页面队列里的 `aborted` 标记。
///
/// 用 `eval_with_callback` + 非阻塞 `try_recv`：发起查询立刻返回，结果在
/// 回调里进 channel，主循环不为此等待（否则采样节奏会被拖成 2 秒一次）。
struct CancelProbe {
    webview: Webview,
    id: String,
    tx: mpsc::Sender<bool>,
    rx: mpsc::Receiver<bool>,
    last_probe: std::time::Instant,
    aborted: bool,
}

impl CancelProbe {
    fn new(webview: &Webview, id: &str) -> Self {
        let (tx, rx) = mpsc::channel::<bool>();
        Self {
            webview: webview.clone(),
            id: id.to_string(),
            tx,
            rx,
            // 立刻探一次（页面可能在认领后马上放弃）。
            last_probe: std::time::Instant::now() - Duration::from_secs(1),
            aborted: false,
        }
    }
}

impl Canceller for CancelProbe {
    fn cancelled(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        // 800ms 一次：取消是人的操作，不需要毫秒级响应；织机一次十几个
        // 并发，探得太勤会白烧 eval。
        if self.last_probe.elapsed() >= Duration::from_millis(800) {
            self.last_probe = std::time::Instant::now();
            let id = self.id.replace('\\', "\\\\").replace('\'', "\\'");
            let script = format!(
                "JSON.stringify(!!(window.__xkGmXhr && window.__xkGmXhr.items['{id}'] && \
                 window.__xkGmXhr.items['{id}'].aborted))"
            );
            let tx = self.tx.clone();
            let _ = self.webview.eval_with_callback(&script, move |value| {
                let _ = tx.send(value.contains("true"));
            });
        }
        while let Ok(flag) = self.rx.try_recv() {
            self.aborted = flag;
        }
        self.aborted
    }
}

/// 下载进度出口。抽象成 trait 是为了能在单测里换成计数器，不用造 Webview。
trait ProgressSink {
    fn report(&mut self, loaded: u64, total: u64);
}

/// 把 curl 已落盘的字节数转给页面的 `onprogress`。
struct ProgressProbe {
    webview: Webview,
    id: String,
    last_loaded: u64,
    last_total: u64,
}

impl ProgressProbe {
    fn new(webview: &Webview, id: &str) -> Self {
        Self {
            webview: webview.clone(),
            id: id.to_string(),
            // 用 u64::MAX 起手，保证第一次 report（哪怕 0 字节）也会发出去。
            last_loaded: u64::MAX,
            last_total: u64::MAX,
        }
    }
}

impl ProgressSink for ProgressProbe {
    fn report(&mut self, loaded: u64, total: u64) {
        if loaded == self.last_loaded && total == self.last_total {
            return;
        }
        self.last_loaded = loaded;
        self.last_total = total;
        let script = format!(
            "(function(){{ var q = window.__xkGmXhr && window.__xkGmXhr.items['{id}']; \
             if (!q || !q.onProgress) return; \
             try {{ q.onProgress({loaded}, {total}); }} catch (e) {{}} }})();",
            id = self.id.replace('\\', "\\\\").replace('\'', "\\'"),
            loaded = loaded,
            total = total,
        );
        let _ = self.webview.eval(&script);
    }
}

/// 用 Windows 自带 curl 执行 GM 跨域请求。
///
/// 有进度出口时不走 `output()` 阻塞等待，而是 spawn 后每 150ms 看一眼
/// 落盘文件大小，把 `loaded/total` 报给页面。`total` 从响应头里的
/// `Content-Length` 取，拿不到就报 0（页面那边对应 `lengthComputable=false`）。
fn curl_xhr(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    data: &str,
    binary: bool,
    mut progress: Option<&mut dyn ProgressSink>,
    mut cancel: Option<&mut dyn Canceller>,
) -> XhrOutcome {
    let seq = GM_XHR_SEQ.fetch_add(1, Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let stem = format!("xk-gm-{}-{}-{}", std::process::id(), stamp, seq);
    let body_path = dir.join(format!("{stem}.bin"));
    let head_path = dir.join(format!("{stem}.hdr"));

    let mut command = std::process::Command::new("curl");
    // GUI 子系统进程拉控制台程序，不带 CREATE_NO_WINDOW 每次都会闪一个黑窗口；
    // 打开网页标签时 GM 请求成片并发，用户看到的就是满屏 curl.exe 窗口。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        // -S 才把错误打到 stderr（-s 会连错误一起吞掉），错误文本要给页面。
        .arg("-sS")
        // 图片 CDN 几乎都是 302 跳到实际地址，不跟就只能拿到一段空壳 HTML。
        .arg("-L")
        .arg("--max-redirs")
        .arg("10")
        .arg("--max-time")
        .arg(if binary { "180" } else { "60" })
        // 头与体分两个文件：二进制体里随时会出现 "\r\n\r\n"，把它们混在
        // 一个输出流里没法可靠切分（旧实现用 `-i`，再被 from_utf8_lossy
        // 过一遍，字节早就废了）。
        .arg("-D")
        .arg(&head_path)
        .arg("-o")
        .arg(&body_path)
        .arg("-X")
        .arg(method)
        .arg(url);
    if !headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("user-agent"))
    {
        command.arg("-A").arg(DEFAULT_UA);
    }
    for (key, value) in headers {
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    if !data.is_empty() && method != "GET" {
        command.arg("--data").arg(data);
    }
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let spawned = command.spawn();
    let mut aborted = false;
    let (success, stderr_text) = match spawned {
        Ok(mut child) => {
            // 边下边报进度：文件大小就是已收字节数。
            let mut exited = None;
            loop {
                // 页面点了 abort：立刻杀子进程，别再白下几十兆。
                if let Some(probe) = &mut cancel {
                    if probe.cancelled() {
                        aborted = true;
                        let _ = child.kill();
                        // 不再 child.wait()：实测在 Windows 上 kill 后等它
                        // 回收会把整条线程挂住（进程已经 TerminateProcess，
                        // 句柄随作用域释放即可）。
                        break;
                    }
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exited = Some(status);
                        break;
                    }
                    Ok(None) => {
                        if let Some(sink) = &mut progress {
                            let loaded = std::fs::metadata(&body_path)
                                .map(|meta| meta.len())
                                .unwrap_or(0);
                            let total = content_length_of(&head_path);
                            sink.report(loaded, total);
                        }
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    Err(_) => break,
                }
            }
            // 取消时不要读 stderr：子进程刚被 kill，管道可能还没彻底关，
            // read_to_string 会一直等 EOF（实测能把测试挂住 60 秒）。
            let stderr_text = if aborted {
                let _ = child.stderr.take();
                String::new()
            } else {
                child
                    .stderr
                    .take()
                    .and_then(|mut pipe| {
                        use std::io::Read;
                        let mut buffer = String::new();
                        pipe.read_to_string(&mut buffer).ok().map(|_| buffer)
                    })
                    .unwrap_or_default()
            };
            // 收尾补一次：最后一块写完后 curl 立刻退出，循环里可能还没轮到
            // 那次 150ms 的采样，进度条就会停在 99%。
            if let Some(sink) = &mut progress {
                let loaded = std::fs::metadata(&body_path)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                let total = content_length_of(&head_path);
                sink.report(loaded, total);
            }
            match exited {
                Some(status) => (status.success(), stderr_text),
                None => (false, stderr_text),
            }
        }
        Err(_) => (false, String::new()),
    };

    let head_raw = std::fs::read(&head_path)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();
    let body_bytes = std::fs::read(&body_path).unwrap_or_default();
    let _ = std::fs::remove_file(&body_path);
    let _ = std::fs::remove_file(&head_path);

    let (status, headers_text) = parse_header_dump(&head_raw);
    let error = !success && status == 0;
    let error_text = if error {
        let detail = stderr_text.trim();
        if detail.is_empty() {
            format!("curl: 请求失败 {url}")
        } else {
            format!("{} {url}", detail.replace('\n', " "))
        }
    } else {
        String::new()
    };
    if binary && body_bytes.len() > GM_XHR_MAX_BINARY {
        return XhrOutcome {
            status,
            text: String::new(),
            base64: String::new(),
            headers: headers_text,
            error: true,
            error_text: format!(
                "响应超过 {}MB 上限，未回传：{url}",
                GM_XHR_MAX_BINARY / 1024 / 1024
            ),
            aborted,
        };
    }
    // 被取消的请求不回半截数据：页面那边只认「已取消」，不会拿残缺字节
    // 去拼图片（半张图比没有更糟）。
    XhrOutcome {
        status,
        text: if aborted {
            String::new()
        } else if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&body_bytes).to_string()
        },
        base64: if !aborted && binary && !body_bytes.is_empty() {
            BASE64.encode(&body_bytes)
        } else {
            String::new()
        },
        headers: headers_text,
        error: if aborted { false } else { error },
        error_text,
        aborted,
    }
}

/// 从 `curl -D` 的头文件里读 `Content-Length`（取最后一次出现的值，
/// 重定向后真正的响应在最后）。读不到就当 0 —— 分块传输本来也没有。
fn content_length_of(path: &std::path::Path) -> u64 {
    let Ok(raw) = std::fs::read(path) else {
        return 0;
    };
    let text = String::from_utf8_lossy(&raw);
    let mut total = 0u64;
    for line in text.lines() {
        let line = line.trim();
        if line.len() < 15 {
            continue;
        }
        let (name, value) = line.split_at(15);
        if name.eq_ignore_ascii_case("content-length:") {
            if let Ok(parsed) = value.trim().parse::<u64>() {
                total = parsed;
            }
        }
    }
    total
}

/// 解析 `curl -D` 抓下来的响应头。跟重定向时这份文件里会有多段，
/// 取**最后一段**（真正的响应）；返回（状态码, 该段头文本）。
fn parse_header_dump(raw: &str) -> (u16, String) {
    let mut status = 0u16;
    let mut headers = String::new();
    for block in raw.split("\r\n\r\n") {
        let block = block.trim_start_matches("\r\n");
        let Some(first) = block.lines().next() else {
            continue;
        };
        let Some(value) = first.trim().strip_prefix("HTTP/") else {
            continue;
        };
        // "HTTP/1.1 200 OK" 去掉前缀是 "1.1 200 OK"，版本号在第 0 位。
        if let Some(code) = value.split_whitespace().nth(1) {
            if let Ok(parsed) = code.parse::<u16>() {
                if parsed > 0 {
                    status = parsed;
                    headers = block.trim_end().to_string();
                }
            }
        }
    }
    (status, headers)
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

/// 页面标题变化（之前完全没接这个事件，标签芯片只能显示 URL）。
fn note_tab_title(label: &str, tab_id: &str, title: &str) {
    with_state(label, |state| {
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.title = title.to_string();
        }
    });
    push_state(label);
}

/// 加载开始/结束（工具栏刷新按钮靠它显示转圈与「停止」）。
fn note_tab_loading(label: &str, tab_id: &str, loading: bool) {
    with_state(label, |state| {
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.loading = loading;
        }
    });
    push_state(label);
}

/// 把最新状态（标签列表/激活项/当前 URL/缩放/加载中/设置）写进工具栏页。
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
        let active = state.tabs.iter().find(|tab| tab.id == state.active);
        let active_url = active.map(|tab| tab.url.clone()).unwrap_or_default();
        serde_json::json!({
            "label": label,
            "tabs": tabs,
            "activeUrl": active_url,
            // 缩放/加载态必须进状态：工具栏原来自己维护 zoomPct，切标签后
            // 显示停在别的标签的百分比上，看着就像「缩放键坏了」。
            "zoom": active.map(|tab| tab.zoom).unwrap_or(1.0),
            "loading": active.map(|tab| tab.loading).unwrap_or(false),
            "settings": state.settings,
        })
        .to_string()
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

fn is_embedded(label: &str) -> bool {
    with_state(label, |state| state.embedded).unwrap_or(false)
}

/// 内置模式：同步 React 占位矩形（逻辑像素），随即重摆。
pub fn embed_sync(label: &str, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    with_state(label, |state| {
        state.embedded = true;
        state.embed_rect = Some((x, y, w, h));
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    reposition(label);
    Ok(())
}

/// 内置模式：整体显/隐（切标签、开模态时用；原生子 webview 盖不住 React 模态）。
pub fn embed_show(label: &str, visible: bool) -> Result<(), String> {
    let parts = with_state(label, |state| {
        (
            state.toolbar.clone(),
            state
                .tabs
                .iter()
                .map(|tab| tab.webview.clone())
                .collect::<Vec<_>>(),
            state.settings_view.clone(),
        )
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    let (toolbar, tabs, settings_view) = parts;
    let apply = |webview: &Webview| {
        if visible {
            let _ = webview.show();
        } else {
            let _ = webview.hide();
        }
    };
    apply(&toolbar);
    for webview in &tabs {
        apply(webview);
    }
    if let Some(view) = &settings_view {
        apply(view);
    }
    Ok(())
}

/// 内置模式：整体拆除（只关 webview，绝不碰宿主窗口），并从状态表摘除。
pub fn embed_close(label: &str) -> Result<(), String> {
    let parts = with_state(label, |state| {
        (
            state.toolbar.clone(),
            state
                .tabs
                .iter()
                .map(|tab| tab.webview.clone())
                .collect::<Vec<_>>(),
            state.settings_view.clone(),
        )
    })
    .ok_or_else(|| "浏览器不存在".to_string())?;
    let (toolbar, tabs, settings_view) = parts;
    let _ = toolbar.close();
    for webview in tabs {
        let _ = webview.close();
    }
    if let Some(view) = settings_view {
        let _ = view.close();
    }
    if let Ok(mut map) = BROWSERS.lock() {
        map.remove(label);
    }
    Ok(())
}

/// 窗口尺寸变化时重排工具栏与全部内容标签。
fn reposition(label: &str) {
    // 死锁修复（2026-11-24）：navigate/eval/reload/set_zoom/set_position/hide/close 这类
    // 调用会同步派发到主线程并阻塞等结果；主线程的页面回调此时可能正等着拿
    // BROWSERS 锁 → 两边互等，整窗白屏卡死。铁律：持锁期间绝不碰 webview，
    // 锁内只把句柄/数据拿出来，动作全部放到锁外执行。
    let Some((window, toolbar, tabs, settings_view, rect)) = with_state(label, |state| {
        (
            state.window.clone(),
            state.toolbar.clone(),
            state
                .tabs
                .iter()
                .map(|tab| tab.webview.clone())
                .collect::<Vec<_>>(),
            state.settings_view.clone(),
            if state.embedded {
                state.embed_rect
            } else {
                None
            },
        )
    })
    else {
        return;
    };
    // 内置模式：占位矩形（逻辑像素 x,y,w,h）驱动摆位；没同步过就先不摆。
    let (origin_x, origin_y, width, height) = match rect {
        Some((x, y, w, h)) => (x, y, w, h),
        None => {
            if is_embedded(label) {
                return;
            }
            // scale_factor / inner_size 同样走主线程，一并放到锁外。
            let scale = window.scale_factor().unwrap_or(1.0);
            let Ok(inner) = window.inner_size() else {
                return;
            };
            (
                0.0,
                0.0,
                inner.width as f64 / scale,
                inner.height as f64 / scale,
            )
        }
    };
    let _ = toolbar.set_position(LogicalPosition::new(origin_x, origin_y));
    let _ = toolbar.set_size(LogicalSize::new(width, TOOLBAR_HEIGHT));
    for webview in tabs {
        let _ = webview.set_position(LogicalPosition::new(origin_x, origin_y + TOOLBAR_HEIGHT));
        let _ = webview.set_size(LogicalSize::new(width, height - TOOLBAR_HEIGHT));
    }
    // 设置浮层铺满整个窗口，窗口拉伸时要跟着长。
    if let Some(view) = settings_view {
        let _ = view.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = view.set_size(LogicalSize::new(width, height));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_name_sanitizes_illegal_windows_chars() {
        // 织机打包名常带 `?` `:`（e-hentai 标题原样），不清洗会让 SetResultFilePath 失败。
        let dest = std::path::Path::new("C:/Users/x/Downloads/[Circle] Title? Vol:1.zip");
        let name = archive_name_from(dest).expect("zip 应该被识别");
        assert!(!name.contains('?'));
        assert!(!name.contains(':'));
        assert!(name.ends_with(".zip"), "扩展名要留住：{name}");
        assert!(name.contains("Circle"), "标题部分要留住：{name}");
    }

    #[test]
    fn media_downloads_are_not_treated_as_archives() {
        // 右键「打开原图并另存」走的是网页下载，不能当成图集去导入。
        assert!(is_media_download(std::path::Path::new("C:/x/原图.jpg")));
        assert!(is_media_download(std::path::Path::new("C:/x/a.PNG")));
        assert!(!is_media_download(std::path::Path::new(
            "C:/x/(C102) [Circle] Title.zip"
        )));
    }

    #[test]
    fn media_download_keeps_suggested_name() {
        // blob URL 没有可用文件名，必须用 WebView2 给的建议名，别退化成 download.bin。
        let url = Url::parse("blob:https://example.com/f59a13ae-8318").unwrap();
        let dest = std::path::Path::new("C:/Users/x/Downloads/原图测试.png");
        assert_eq!(safe_download_name(dest, &url), "原图测试.png");
    }

    #[test]
    fn archive_name_rejects_non_archives() {
        assert!(archive_name_from(std::path::Path::new("C:/a/b.png")).is_none());
        // blob 下载丢了扩展名的情况：交给魔数嗅探兜底，这里不硬猜。
        assert!(archive_name_from(std::path::Path::new("C:/a/6f1c-2b")).is_none());
    }

    #[test]
    fn parse_header_dump_takes_final_response() {
        // curl -L 跟一次 302 后，头文件里是两段，要取最后一段的状态与头。
        let raw = "HTTP/1.1 302 Found\r\nLocation: https://b/\r\n\r\n\
                   HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\n\r\n";
        let (status, headers) = parse_header_dump(raw);
        assert_eq!(status, 200, "取最后一段响应的状态码");
        assert!(headers.contains("200 OK"));
        assert!(headers.contains("Content-Type: image/jpeg"));
        assert!(!headers.contains("Location:"), "不该残留重定向那段的头");
    }

    #[test]
    fn parse_header_dump_handles_http2_and_continue() {
        let raw = "HTTP/1.1 100 Continue\r\n\r\nHTTP/2 206 Partial Content\r\nContent-Length: 12\r\n\r\n";
        let (status, headers) = parse_header_dump(raw);
        assert_eq!(status, 206);
        assert!(headers.starts_with("HTTP/2 206"));
    }

    #[test]
    fn parse_header_dump_on_garbage_returns_zero() {
        assert_eq!(parse_header_dump(""), (0, String::new()));
        assert_eq!(parse_header_dump("not a header"), (0, String::new()));
    }

    #[test]
    fn binary_body_survives_base64_roundtrip() {
        // 二进制体里带 CRLFCRLF（旧实现按这个切头/体，必然把图切坏）。
        let bytes: Vec<u8> = (0u8..=255).chain([13, 10, 13, 10, 0, 255]).collect();
        let encoded = BASE64.encode(&bytes);
        let decoded = BASE64.decode(encoded.as_bytes()).expect("base64 应可解码");
        assert_eq!(decoded, bytes);
        assert!(encoded.chars().all(|c| c.is_ascii()), "回传串必须是纯 ASCII");
    }

    /// 起一个只回一次的极简 HTTP 服务，返回（地址, 收工句柄）。
    fn serve_once(body: Vec<u8>, content_type: &str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑本地端口");
        let port = listener.local_addr().unwrap().port();
        let content_type = content_type.to_string();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut scratch = [0u8; 2048];
                let _ = stream.read(&mut scratch);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/page.jpg"), handle)
    }

    fn curl_ready() -> bool {
        std::process::Command::new("curl")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[test]
    fn curl_xhr_keeps_binary_bytes_intact() {
        if !curl_ready() {
            return; // Windows 10+ 自带 curl；没有就跳过，不算失败
        }
        // 体里塞 JPEG 头、CRLFCRLF、非 UTF-8 字节：旧实现 `-i` + 按 "\r\n\r\n"
        // 切分 + from_utf8_lossy，这三步任一步都足以把这几个字节弄坏。
        let body: Vec<u8> = [
            vec![0xff, 0xd8, 0xff, 0xe0],
            b"\r\n\r\n".to_vec(),
            vec![0x00, 0xc8, 0x0d, 0x0a],
        ]
        .concat();
        let (url, server) = serve_once(body.clone(), "image/jpeg");
        let outcome = curl_xhr(&url, "GET", &[], "", true, None, None);
        assert_eq!(outcome.status, 200);
        assert!(!outcome.error, "本地 200 不该判成错误");
        assert!(outcome.text.is_empty(), "二进制请求不该回文本");
        assert!(outcome.headers.contains("image/jpeg"));
        let decoded = BASE64.decode(outcome.base64.as_bytes()).expect("base64 可解");
        assert_eq!(decoded, body, "字节必须一字节不差地回传");
        server.join().unwrap();
    }

    #[test]
    fn curl_xhr_returns_text_for_plain_requests() {
        if !curl_ready() {
            return;
        }
        let (url, server) = serve_once(b"<html>ok</html>".to_vec(), "text/html");
        let outcome = curl_xhr(&url, "GET", &[], "", false, None, None);
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.text, "<html>ok</html>");
        assert!(outcome.base64.is_empty(), "文本请求不必编 base64");
        server.join().unwrap();
    }

    /// 慢慢吐字节的服务端：分块发出、块间停顿，用来验证下载中的进度上报。
    fn serve_slow(chunks: Vec<Vec<u8>>, gap: Duration) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑本地端口");
        let port = listener.local_addr().unwrap().port();
        let total: usize = chunks.iter().map(|chunk| chunk.len()).sum();
        let handle = std::thread::spawn(move || {
            // accept 必须有上限：客户端可能在连接建立前就被取消，裸 accept
            // 会把整个测试挂死（实测 60 秒都等不回来）。
            listener.set_nonblocking(true).ok();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(ref error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        if std::time::Instant::now() > deadline {
                            break None;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break None,
                }
            };
            let Some(mut stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            let mut scratch = [0u8; 2048];
            let _ = stream.read(&mut scratch);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\n\
                 Content-Length: {total}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            for (index, chunk) in chunks.iter().enumerate() {
                if index > 0 {
                    std::thread::sleep(gap);
                }
                let _ = stream.write_all(chunk);
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/slow.jpg"), handle)
    }

    struct CollectedProgress(Vec<(u64, u64)>);

    impl ProgressSink for CollectedProgress {
        fn report(&mut self, loaded: u64, total: u64) {
            self.0.push((loaded, total));
        }
    }

    #[test]
    fn curl_xhr_reports_progress_while_downloading() {
        if !curl_ready() {
            return;
        }
        // 三块、块间 400ms：150ms 一次的探针一定能踩到中间态。
        let chunks = vec![vec![7u8; 8192], vec![8u8; 8192], vec![9u8; 8192]];
        let total = 8192u64 * 3;
        let (url, server) = serve_slow(chunks, Duration::from_millis(400));
        let mut sink = CollectedProgress(Vec::new());
        let outcome = curl_xhr(&url, "GET", &[], "", true, Some(&mut sink), None);
        server.join().unwrap();

        assert_eq!(outcome.status, 200, "慢速服务也应返回 200");
        let seen = &sink.0;
        assert!(
            seen.len() >= 2,
            "下载过程中应至少报两次进度，实际 {} 次：{:?}",
            seen.len(),
            seen
        );
        let mut last = 0u64;
        for (loaded, _) in seen {
            assert!(*loaded >= last, "进度不能倒退：{:?}", seen);
            last = *loaded;
        }
        assert_eq!(
            seen.last().map(|(loaded, _)| *loaded),
            Some(total),
            "最后一次进度应等于总字节数：{:?}",
            seen
        );
        assert!(
            seen.iter().any(|(loaded, reported_total)| *loaded < total
                && *reported_total == total),
            "中途应能报出 Content-Length 作为 total：{:?}",
            seen
        );
    }

    #[test]
    fn curl_xhr_reports_error_text_with_url() {
        if !curl_ready() {
            return;
        }
        // 连不上的端口：curl 立刻报错，错文里要带 URL，供脚本提取域名。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}/gone.jpg");
        let outcome = curl_xhr(&url, "GET", &[], "", false, None, None);
        assert!(outcome.error, "连不上应判成传输失败");
        assert_eq!(outcome.status, 0);
        assert!(
            outcome.error_text.contains(&url),
            "错误文本要带 URL：{}",
            outcome.error_text
        );
    }

    #[test]
    fn gm_claim_script_marks_running() {
        // 认领脚本必须自己把 pending → running 并带回内容：轮询线程只管发请求，
        // 否则同一批条目会被反复取走（重复下载同一张图）。
        assert!(GM_CLAIM_SCRIPT.contains("state = 'running'"));
        assert!(GM_CLAIM_SCRIPT.contains("state === 'pending'"));
        assert!(GM_CLAIM_SCRIPT.contains("__xkGmXhr"));
        // 已取消的条目不该再被认领（用户点了取消还继续下载是最气人的）。
        assert!(GM_CLAIM_SCRIPT.contains("!it.aborted"));
    }

    /// 第 n 次被问「还要不要」时回答「不要了」。
    struct CancelAfterN {
        ask_count: usize,
        cancel_at: usize,
    }

    impl Canceller for CancelAfterN {
        fn cancelled(&mut self) -> bool {
            self.ask_count += 1;
            self.ask_count >= self.cancel_at
        }
    }

    #[test]
    fn curl_xhr_kills_curl_when_page_aborts() {
        if !curl_ready() {
            return;
        }
        // 三块、块间 500ms：不取消的话至少要 1 秒才下完。
        let chunks = vec![vec![3u8; 8192], vec![4u8; 8192], vec![5u8; 8192]];
        let (url, server) = serve_slow(chunks, Duration::from_millis(500));
        let started = std::time::Instant::now();
        // 第 3 次问（约 300ms，curl 已连上并收到第一块）才取消——第一次就
        // 取消的话 curl 还没建立连接，等于没测到「下载中途取消」。
        let mut cancel = CancelAfterN {
            ask_count: 0,
            cancel_at: 3,
        };
        let outcome = curl_xhr(&url, "GET", &[], "", true, None, Some(&mut cancel));
        let elapsed = started.elapsed();
        let _ = server.join();

        assert!(outcome.aborted, "页面取消后应标记 aborted");
        assert!(
            outcome.base64.is_empty() && outcome.text.is_empty(),
            "取消的请求不能回半截数据"
        );
        assert!(!outcome.error, "取消不是网络错误，别触发 onerror");
        assert!(
            elapsed < Duration::from_millis(1200),
            "应在第一块之后就杀进程，实际耗时 {:?}",
            elapsed
        );
    }

    #[test]
    fn address_target_tells_url_from_query() {
        let settings = BrowserSettings::default();
        assert_eq!(
            address_target("e-hentai.org/g/1/2/", &settings),
            "https://e-hentai.org/g/1/2/"
        );
        assert_eq!(
            address_target("https://e-hentai.org", &settings),
            "https://e-hentai.org"
        );
        assert_eq!(address_target("localhost:3000", &settings), "https://localhost:3000");
        // 带空格的词当搜索：以前会拼成 https://xxx 漫画 这种坏地址。
        assert_eq!(
            address_target("百合 漫画", &settings),
            "https://www.bing.com/search?q=%E7%99%BE%E5%90%88+%E6%BC%AB%E7%94%BB"
        );
        // 不像域名的单词也走搜索。
        assert_eq!(
            address_target("nhentai", &settings),
            "https://www.bing.com/search?q=nhentai"
        );
    }

    #[test]
    fn browser_settings_sanitizes_bad_values() {
        let broken = BrowserSettings {
            home: "  ".to_string(),
            new_tab: "weird".to_string(),
            search_url: "https://x/?no-placeholder".to_string(),
            zoom_step: 9.0,
            shortcuts: false,
            show_titles: false,
            tabs_width: 3.0,
        }
        .sanitized();
        assert_eq!(broken.home, BrowserSettings::default().home, "空主页回退默认");
        assert_eq!(broken.new_tab, "home", "非法新建行为回退主页");
        assert!(
            broken.search_url.contains("{q}"),
            "缺占位符的搜索模板必须回退：{}",
            broken.search_url
        );
        assert_eq!(broken.zoom_step, 0.5, "步进上限 50%");
        assert_eq!(broken.tabs_width, 0.7, "标签栏上限 70%");
        // 布尔开关原样保留（用户明确关掉的不能被“修正”回去）。
        assert!(!broken.shortcuts);
        assert!(!broken.show_titles);
    }
}
