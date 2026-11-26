//! 站点登录态的自动保存与自动回填。
//!
//! 管理员 2026-11-26：18comic 这类站点的登录态是**会话 cookie**（没有过期时间），
//! WebView2 关掉就清空，所以「登录过、关掉画匣、再打开又要重登」永远修不好——
//! 除非我们自己在页面加载完成时把这份 cookie 抓下来加密落库，下次到该站再写回去。
//!
//! 这条链路不依赖用户手动点「保存登录态」：
//! 1. 页面加载完成 → 该站 cookie 非空 → 指纹变了就写库（DPAPI 加密）；
//! 2. 页面加载完成 → 该站 cookie 为空 → 库里有快照就写回 webview 并 reload（每标签每站只做一次）。
//!
//! 只对内置名单里的图站生效，避免把随便什么站的 cookie 都囤进库里。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, Webview};

/// 会做自动登录态管理的站点（主域后缀匹配）。
const LOGIN_SITES: &[&str] = &[
    "18comic.vip",
    "18comic.org",
    "18comic.me",
    "18comic.art",
    "jmcomic.me",
    "jmcomic.mobi",
    "e-hentai.org",
    "exhentai.org",
    "nhentai.net",
    "wnacg.com",
    "wnacg.org",
    "wnacg01.com",
];

/// 已经尝试过回填的 (webview label, host)：保证每个标签对每个站只回填一次，
/// 否则回填 → reload → 又触发加载完成 → 再回填，会自己打转。
static RESTORED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// 上次落库的内容指纹，避免每加载一个页面就把同一条记录重写一遍。
static LAST_SAVED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
/// 每个站点的上次同步时间：图站（尤其漫画织机）一次阅读会触发很多次加载，
/// 每次都去读 cookie 会把主线程拖慢，这里做 4 秒节流。
static LAST_SYNC: Mutex<Option<HashMap<String, std::time::Instant>>> = Mutex::new(None);

fn throttled(host: &str) -> bool {
    let mut guard = match LAST_SYNC.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    };
    let map = guard.get_or_insert_with(HashMap::new);
    let now = std::time::Instant::now();
    if let Some(last) = map.get(host) {
        if now.duration_since(*last) < Duration::from_secs(4) {
            return true;
        }
    }
    map.insert(host.to_string(), now);
    false
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn is_login_site(host: &str) -> bool {
    LOGIN_SITES
        .iter()
        .any(|site| host == *site || host.ends_with(&format!(".{site}")))
}

/// 第一次处理 (label, host) 时返回 true。
fn first_time(label: &str, host: &str) -> bool {
    let key = format!("{label}|{host}");
    let mut guard = match RESTORED.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    };
    guard.get_or_insert_with(HashSet::new).insert(key)
}

fn fingerprint_changed(host: &str, cookies: &str) -> bool {
    let print = fnv1a(cookies);
    let mut guard = match LAST_SAVED.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    };
    let map = guard.get_or_insert_with(HashMap::new);
    if map.get(host).copied() == Some(print) {
        return false;
    }
    map.insert(host.to_string(), print);
    true
}

/// 页面加载完成回调（由 browser 的 on_page_load 转发进来）。
///
/// 这里立刻 spawn 一个线程：cookie 读写会同步派发到 WebView2 主线程，
/// 在页面加载回调里直接调有互等的风险（项目里踩过一次整窗白屏）。
pub fn on_page_loaded(webview: &Webview, url: &str) {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return;
    }
    let Some(host) = parsed.host_str().map(|value| value.to_ascii_lowercase()) else {
        return;
    };
    if host.is_empty() || !is_login_site(&host) {
        return;
    }
    let label = webview.label().to_string();
    let webview = webview.clone();
    std::thread::spawn(move || {
        // 给页面的登录脚本一点时间把 cookie 写完（有的站是 XHR 里 set-cookie）。
        std::thread::sleep(Duration::from_millis(400));
        sync(&label, &webview, &parsed, &host);
    });
}

fn sync(label: &str, webview: &Webview, parsed: &tauri::Url, host: &str) {
    if throttled(host) {
        return;
    }
    let cookies = read_cookies(webview, parsed);
    let app = webview.app_handle();

    if !has_login_cookie(host, &cookies) {
        // 浏览器里没有 → 看库里的快照，回填一次再刷新（每个标签每站只回填一次）。
        if !first_time(label, host) {
            return;
        }
        let Some(saved) = crate::stored_cookie_for_host(app, host) else {
            return;
        };
        if saved.trim().is_empty() {
            return;
        }
        let written = apply_cookies(webview, host, &saved);
        if written > 0 {
            // 下载链路缓存里可能正压着「没登录」的空值，必须清掉。
            crate::netdl::clear_cookie_cache();
            // 只有页面还停在原地才刷新：用户已经点走/后退时再 reload，
            // 会把他刚发起的导航顶掉，感觉就是「前进后退不灵」。
            let still_here = webview
                .url()
                .map(|current| current.as_str() == parsed.as_str())
                .unwrap_or(false);
            if still_here {
                let _ = webview.reload();
            }
        }
        return;
    }

    // 有登录态 → 自动存一份（内容没变就不重复写库）。
    if !fingerprint_changed(host, &cookies) {
        return;
    }
    if crate::save_site_login_snapshot(app, host, &cookies, "浏览时自动保存").is_ok() {
        // 登录态更新了，下次下载要拿新的 cookie。
        crate::netdl::clear_cookie_cache();
    }
}

/// 各站「算登录了」的标志 cookie。
///
/// 回填的判定不能是「有没有任意 cookie」：`theme` / `_ga` / `cf_chl_rc_ni`
/// 这些是**持久** cookie，重启后还留着，会把「实际已掉线」误判成已登录，
/// 于是真正的登录 cookie（AVS 这类会话 cookie）永远没机会写回去——
/// 管理员 2026-11-26 晚：明明给过 cookie，打开还是登录页。
fn login_markers(host: &str) -> &'static [&'static str] {
    if host.contains("18comic") || host.contains("jmcomic") {
        &["AVS"]
    } else if host.contains("exhentai") || host.contains("e-hentai") {
        &["ipb_member_id", "ipb_pass_hash"]
    } else if host.contains("nhentai") {
        &["sessionid", "access_token"]
    } else {
        &[]
    }
}

/// 该站当前算不算登录态在。名单外的站点退回「有任意 cookie 就算」。
fn has_login_cookie(host: &str, cookies: &str) -> bool {
    let markers = login_markers(host);
    if markers.is_empty() {
        return !cookies.trim().is_empty();
    }
    markers.iter().any(|marker| {
        cookies.split(';').any(|part| {
            let piece = part.trim();
            piece.len() > marker.len() + 1
                && piece[..marker.len()].eq_ignore_ascii_case(marker)
                && piece.as_bytes()[marker.len()] == b'='
        })
    })
}

fn read_cookies(webview: &Webview, parsed: &tauri::Url) -> String {
    let Ok(list) = webview.cookies_for_url(parsed.clone()) else {
        return String::new();
    };
    list.iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 把一段 `name=value; name=value` 写进 webview（带点/裸域各写一遍）。
/// 统一设 180 天过期：不设就是会话 cookie，画匣一重启又没了。
fn apply_cookies(webview: &Webview, host: &str, header: &str) -> usize {
    let hosts = [format!(".{host}"), host.to_string()];
    let mut written = 0usize;
    for part in header.split([';', '\n', '\r']) {
        let piece = part.trim();
        if piece.is_empty() {
            continue;
        }
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
        let Some((name, value)) = piece.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        for target in &hosts {
            let mut cookie = cookie::Cookie::new(name.to_string(), value.to_string());
            cookie.set_domain(target.clone());
            cookie.set_path("/");
            cookie.set_max_age(cookie::time::Duration::days(180));
            if webview.set_cookie(cookie).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// 把用户粘贴的 Cookie 直接存进加密库（不要求浏览器开着），并尽量写进已打开的标签。
/// 这是「我直接把 cookie 给你」那条路：粘贴即生效，下载立刻能用。
pub fn import_cookie_text(
    app: &AppHandle,
    host: &str,
    cookie_text: &str,
) -> Result<usize, String> {
    let host = crate::db::normalize_host(host);
    if host.is_empty() {
        return Err("请填写站点域名，例如 18comic.vip".to_string());
    }
    // 归一成 `name=value; name=value`：用户可能粘的是整段请求头。
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in cookie_text.split([';', '\n', '\r']) {
        let piece = part.trim();
        if piece.is_empty() {
            continue;
        }
        let lower = piece.to_ascii_lowercase();
        if lower.starts_with("path=")
            || lower.starts_with("domain=")
            || lower.starts_with("expires=")
            || lower.starts_with("max-age=")
            || lower == "httponly"
            || lower == "secure"
            || lower.starts_with("samesite=")
            || lower.starts_with("cookie:")
        {
            continue;
        }
        if let Some((name, value)) = piece.split_once('=') {
            let (name, value) = (name.trim(), value.trim());
            if !name.is_empty() && !value.is_empty() {
                pairs.push((name.to_string(), value.to_string()));
            }
        }
    }
    if pairs.is_empty() {
        return Err("没解析出任何 cookie：请粘 name=value 形式的内容".to_string());
    }
    let snapshot = pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");

    crate::save_site_login_snapshot(app, &host, &snapshot, "手动粘贴 Cookie")?;
    crate::netdl::clear_cookie_cache();

    // 顺手写进已经打开的标签：不动主界面，只认内容标签。
    for (label, webview) in app.webviews() {
        if !label.contains("-tab-") {
            continue;
        }
        let Ok(url) = webview.url() else {
            continue;
        };
        let current = url.host_str().unwrap_or("").to_ascii_lowercase();
        if current == host || current.ends_with(&format!(".{host}")) {
            let _ = apply_cookies(&webview, &host, &snapshot);
            let mut refresh = url.clone();
            refresh.set_fragment(None);
            let _ = webview.navigate(refresh);
        }
    }
    Ok(pairs.len())
}

/// 手动「保存当前站点登录态」：从活着的标签里抓一次（保留原有命令语义）。
pub fn capture_from_tabs(app: &AppHandle, host: &str) -> String {
    let mut best = String::new();
    for (label, webview) in app.webviews() {
        if !label.contains("-tab-") {
            continue;
        }
        let Ok(url) = webview.url() else {
            continue;
        };
        let current = url.host_str().unwrap_or("").to_ascii_lowercase();
        if current == host || current.ends_with(&format!(".{host}")) {
            let cookies = read_cookies(&webview, &url);
            if cookies.split(';').count() > best.split(';').count() {
                best = cookies;
            }
        }
    }
    best
}
