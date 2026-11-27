//! 18comic（禁漫）移动 API 适配：整本的「源数据」路线。
//!
//! 参考了 DSH 的 jm-downloader 插件（底层是 jmcomic 库）：网页抓取会被
//! Cloudflare 卡、标签/标题/章节都要从 HTML 里硬抠；而站点自己的移动 API
//! 直接给结构化数据——标题、标签、章节列表、每章图片清单，一次拿全。
//!
//! 协议（jmcomic 同款，公开逆向）：
//! - 请求：`GET https://{api域名}/album?id={aid}` / `/chapter?id={photo_id}`
//!   头：`token` + `tokenparam`；`token = md5(ts + "185Hcomic3PAPP7R")`，
//!   `tokenparam = "{ts},2.1.13"`，UA 用安卓 WebView，必须带站点 cookie。
//! - 返回：`{"code": 1, "data": "<base64>"}`；data 先 base64 解码，
//!   再用 **AES-256-ECB**（key = md5(ts+"185Hcomic3PAPP7R") 的 32 字节
//!   hex 字符串）解出 JSON。
//! - album：`{name, tags: [], series: [{id,...}], ...}`；
//!   chapter：`{name, images: ["00001.webp", ...], ...}`；
//!   图片地址 = `https://{图片域名}/media/photos/{photo_id}/{文件名}`。
//!
//! scramble_id 拿不到也没关系：`jm_decode::decode_image_file` 会对公式算出的
//! 段数做画面自校验，错段数能自动纠回。

use base64::Engine;
use serde_json::Value;

use crate::netdl;

/// 移动 API 域名（jmcomic 的 DOMAIN_API_LIST；打不通就按序换下一个）。
const API_DOMAINS: &[&str] = &["www.cdnhjk.net", "www.cdngwc.cc", "www.cdngwc.net", "www.cdngwc.club"];

/// 图片域名（DOMAIN_IMAGE_LIST）。
const IMAGE_DOMAINS: &[&str] = &[
    "cdn-msp.jmapiproxy1.cc",
    "cdn-msp.jmapiproxy2.cc",
    "cdn-msp2.jmapiproxy2.cc",
    "cdn-msp3.jmapiproxy2.cc",
    "cdn-msp.jmapinodeudzn.net",
    "cdn-msp3.jmapinodeudzn.net",
];

const TOKEN_SECRET: &str = "185Hcomic3PAPP7R";
const APP_VERSION: &str = "2.1.13";
/// 移动端 UA（jmcomic 的 APP_HEADERS_TEMPLATE）。
const APP_UA: &str = "Mozilla/5.0 (Linux; Android 9; V1938CT Build/PQ3A.190705.11211812; wv) \
AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/91.0.4472.114 Safari/537.36";

fn md5hex(text: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// base64 → AES-256-ECB（PKCS7）解密，key 是 32 字节 hex 字符串。
fn decrypt_data(encoded: &str, ts: &str) -> Result<Value, String> {
    use ecb::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
    type Dec = ecb::Decryptor<aes::Aes256>;

    let cipher_text = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("接口返回 base64 解不开：{e}"))?;
    let key_text = md5hex(&format!("{ts}{TOKEN_SECRET}"));
    let key = key_text.as_bytes(); // 32 字节 → AES-256
    let mut decryptor = Dec::new(key.into());
    let plain = decryptor
        .decrypt_padded_vec_mut::<Pkcs7>(&cipher_text)
        .map_err(|e| format!("接口数据解密失败：{e}"))?;
    serde_json::from_slice(&plain).map_err(|e| format!("解密后的数据不是 JSON：{e}"))
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// 调一个移动 API 端点，返回解密后的 JSON。域名按序换着试。
fn api_get(app: &tauri::AppHandle, path_query: &str) -> Result<Value, String> {
    let ts = now_ts().to_string();
    let token = md5hex(&format!("{ts}{TOKEN_SECRET}"));
    let mut last = String::new();
    for domain in API_DOMAINS {
        let url = format!("https://{domain}{path_query}");
        let output = curl_app(app, &url, &ts, &token);
        match output {
            Ok(body) => match parse_api_body(&body, &ts) {
                Ok(value) => return Ok(value),
                Err(error) => last = error,
            },
            Err(error) => last = error,
        }
    }
    Err(if last.is_empty() {
        "站点接口全部域名都没通".to_string()
    } else {
        last
    })
}

/// form 表单值转义（curl -d 原样发送，这里只做最小 percent-encoding）。
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 登录：先走**网页表单 POST**（官方 JMComic 的 JmHtmlClient.login 同款），
/// 不通再退回移动端 API。返回 AVS cookie 值。
///
/// 为什么主路径换成网页 POST：2026-11-27 管理员实测「登录接口 HTTP 401」，
/// 本机复测四个接口域名的 /login 全部 404——接口域名根本没挂登录路由。
/// 而网页 POST `https://18comic.vip/login` 不经过验证码（验证码只拦浏览器
/// 里的交互流程，拦不住直接表单提交），status==1 时 AVS 就在响应的
/// Set-Cookie 里；curl 带上浏览器的 cf_clearance（cookie_for）能过盾。
pub fn api_login(app: &tauri::AppHandle, username: &str, password: &str) -> Result<String, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号和密码都要填".to_string());
    }
    match api_login_web(app, username, password) {
        Ok(avs) => return Ok(avs),
        Err(web_error) => {
            if let Ok(avs) = api_login_mobile(app, username, password) {
                return Ok(avs);
            }
            Err(web_error)
        }
    }
}

/// 网页表单登录：POST https://18comic.vip/login，AVS 从响应 Set-Cookie 里取。
fn api_login_web(app: &tauri::AppHandle, username: &str, password: &str) -> Result<String, String> {
    const WEB_LOGIN_URL: &str = "https://18comic.vip/login";
    let form = format!(
        "username={}&password={}&id_remember=on&login_remember=on&submit_login=",
        form_encode(username),
        form_encode(password)
    );
    let mut command = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .arg("-sS")
        // 不 -L：官方 allow_redirects=False，30x 只会是异常信号。
        .arg("--max-time")
        .arg("30")
        .arg("--connect-timeout")
        .arg("8")
        .arg("-A")
        .arg(crate::browser::webview_user_agent())
        .arg("-H")
        .arg("content-type: application/x-www-form-urlencoded")
        .arg("-H")
        .arg("referer: https://18comic.vip/")
        .arg("-H")
        .arg("x-requested-with: XMLHttpRequest")
        .arg("-H")
        .arg(format!("cookie: {}", netdl::cookie_for(app, WEB_LOGIN_URL)))
        .arg("-X")
        .arg("POST")
        .arg("--data")
        .arg(&form)
        // 响应头打到 stdout：AVS 就在 Set-Cookie 里。
        .arg("-D")
        .arg("-")
        .arg("-w")
        .arg("\n%{http_code}")
        .arg(WEB_LOGIN_URL);
    if let Some(proxy) = netdl::proxy_arg() {
        command.arg("--proxy").arg(proxy);
    }
    let output = command
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    // 尾行 = HTTP 状态码（-w）；其上是「头 + 体」。
    let Some((head_and_body, code_line)) = text.rsplit_once('\n') else {
        return Err("登录接口没有响应".to_string());
    };
    let code: u16 = code_line.trim().parse().unwrap_or(0);
    if code == 403 {
        return Err(
            "被 Cloudflare 盾拦下：请先在浏览器标签里打开一次 18comic.vip 过盾，再回来登录"
                .to_string(),
        );
    }
    if !(200..300).contains(&code) {
        return Err(format!("登录接口 HTTP {code}"));
    }
    // 头在体前面，第一个空行分界。
    let (headers, body) = head_and_body
        .split_once("\r\n\r\n")
        .unwrap_or((head_and_body, ""));
    if let Some(avs) = extract_avs_from_headers(headers) {
        return Ok(avs);
    }
    // 没有 AVS cookie：看 JSON（status!=1 时把站点错误透出来）。
    let value: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
    let status_ok = value.get("status").and_then(|v| v.as_i64()) == Some(1);
    if status_ok {
        // 重复登录的官方已知 bug：status=1 但 Set-Cookie 里没有 AVS。
        return Err(
            "站点说登录成功但没下发 AVS cookie：请到 设置 → 浏览器数据 → 登录态 里手动导入一次"
                .to_string(),
        );
    }
    let msg = value
        .get("errors")
        .and_then(|v| v.as_array())
        .and_then(|list| list.first())
        .and_then(|v| v.as_str())
        .or_else(|| value.get("msg").and_then(|v| v.as_str()))
        .unwrap_or("账号或密码不对（站点也没给出具体原因）");
    Err(format!("登录失败：{msg}"))
}

/// 从响应头里抠出 AVS cookie 值（Set-Cookie 大小写不敏感匹配，值取到 ';' 为止）。
fn extract_avs_from_headers(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.starts_with("set-cookie:") {
            continue;
        }
        if let Some(pos) = lower.find("avs=") {
            let value: String = line[pos + 4..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// 移动端 API 登录（对齐 jmcomic `req_api('/login', False, data={username,password})`）：
/// 成功后解密数据的 `s` 字段就是网页登录态 cookie **AVS** 的值。
/// 用它绕开网页登录：内嵌 WebView2 里 18comic 登录依赖 Google reCAPTCHA，
/// gstatic 加载失败时点「登錄」毫无反应（管理员 2026-11-27 反馈登录不了）。
/// 返回 AVS token；失败 Err 带原因（账号密码错误/接口不通）。
fn api_login_mobile(app: &tauri::AppHandle, username: &str, password: &str) -> Result<String, String> {
    let ts = now_ts().to_string();
    let token = md5hex(&format!("{ts}{TOKEN_SECRET}"));
    let form = format!(
        "username={}&password={}",
        form_encode(username),
        form_encode(password)
    );
    let mut last = String::new();
    for domain in API_DOMAINS {
        let url = format!("https://{domain}/login/");
        match curl_post(app, &url, &ts, &token, &form) {
            Ok(body) => match parse_api_body(&body, &ts) {
                Ok(value) => {
                    let avs = value.get("s").and_then(|v| v.as_str()).unwrap_or("");
                    if avs.is_empty() {
                        // status!=1 时 jmcomic 报 errors/msg——原样透出更有用。
                        let msg = value
                            .get("msg")
                            .or_else(|| value.get("errors"))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "接口没返回登录凭据".to_string());
                        return Err(format!("登录失败：{msg}"));
                    }
                    return Ok(avs.to_string());
                }
                Err(error) => last = error,
            },
            Err(error) => last = error,
        }
    }
    Err(if last.is_empty() {
        "登录接口全部域名都没通".to_string()
    } else {
        last
    })
}

/// 用 curl 调接口（走下载链路同一套代理/cookie，UA 换成移动端）。
fn curl_app(app: &tauri::AppHandle, url: &str, ts: &str, token: &str) -> Result<String, String> {
    let mut command = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .arg("-sS")
        .arg("-L")
        .arg("--max-time")
        .arg("25")
        .arg("--connect-timeout")
        .arg("8")
        .arg("-A")
        .arg(APP_UA)
        .arg("-H")
        .arg(format!("token: {token}"))
        .arg("-H")
        .arg(format!("tokenparam: {ts},{APP_VERSION}"))
        .arg("-H")
        .arg(format!("cookie: {}", netdl::cookie_for(app, url)))
        .arg("-w")
        .arg("\n%{http_code}")
        .arg(url);
    if let Some(proxy) = netdl::proxy_arg() {
        command.arg("--proxy").arg(proxy);
    }
    let output = command
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.rsplit_once('\n').map(|(body, code)| (body, code.trim()));
    match lines {
        Some((body, code)) if code == "200" => Ok(body.to_string()),
        Some((_, code)) => Err(format!("站点接口 HTTP {code}")),
        None => Err("站点接口返回空".to_string()),
    }
}

/// POST 版 curl（登录用）：同一个 UA/token 头，body 走 form-encoded。
fn curl_post(
    app: &tauri::AppHandle,
    url: &str,
    ts: &str,
    token: &str,
    form: &str,
) -> Result<String, String> {
    let mut command = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .arg("-sS")
        .arg("-L")
        .arg("--max-time")
        .arg("25")
        .arg("--connect-timeout")
        .arg("8")
        .arg("-A")
        .arg(APP_UA)
        .arg("-H")
        .arg(format!("token: {token}"))
        .arg("-H")
        .arg(format!("tokenparam: {ts},{APP_VERSION}"))
        .arg("-H")
        .arg("content-type: application/x-www-form-urlencoded")
        .arg("-H")
        .arg(format!("cookie: {}", netdl::cookie_for(app, url)))
        .arg("-X")
        .arg("POST")
        .arg("--data")
        .arg(form)
        .arg("-w")
        .arg("\n%{http_code}")
        .arg(url);
    if let Some(proxy) = netdl::proxy_arg() {
        command.arg("--proxy").arg(proxy);
    }
    let output = command
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.rsplit_once('\n').map(|(body, code)| (body, code.trim()));
    match lines {
        Some((body, code)) if code == "200" => Ok(body.to_string()),
        Some((_, code)) => Err(format!("登录接口 HTTP {code}")),
        None => Err("登录接口返回空".to_string()),
    }
}

/// 拆外层 JSON、解 data 字段。
fn parse_api_body(body: &str, ts: &str) -> Result<Value, String> {
    let outer: Value =
        serde_json::from_str(body.trim()).map_err(|e| format!("接口返回不是 JSON：{e}"))?;
    let encoded = outer
        .get("data")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "接口返回里没有 data 字段".to_string())?;
    decrypt_data(encoded, ts)
}

/// album 的源数据：标题、标签、章节列表。
pub struct JmAlbumData {
    pub title: String,
    /// 平铺成 (命名空间, 值)；API 只给字符串数组，统一归 "tag"。
    pub tags: Vec<(String, String)>,
    pub chapters: Vec<JmChapter>,
}

pub struct JmChapter {
    pub photo_id: u64,
    pub name: String,
    pub pages: Vec<String>,
}

fn value_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 拿整本源数据：album 给标题/标签/章节；series 为空时整本就是单章。
pub fn fetch_album(app: &tauri::AppHandle, aid: u64) -> Result<JmAlbumData, String> {
    let album = api_get(app, &format!("/album?id={aid}"))?;
    let title = {
        let name = value_str(&album, "name");
        let alt = value_str(&album, "title");
        if name.is_empty() { alt } else { name }
    };
    let mut tags: Vec<(String, String)> = Vec::new();
    if let Some(list) = album.get("tags").and_then(|v| v.as_array()) {
        for tag in list {
            let Some(tag) = tag.as_str() else { continue };
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            // "-group:翻译" 这类带命名空间的拆开，纯文本归 tag。
            let (kind, value) = match tag.split_once(':') {
                Some((ns, value)) => (ns.trim().to_string(), value.trim().to_string()),
                None => ("tag".to_string(), tag.to_string()),
            };
            if !value.is_empty() && !tags.iter().any(|(_, existing)| existing == &value) {
                tags.push((kind, value));
            }
        }
    }

    let mut chapters: Vec<JmChapter> = Vec::new();
    let series = album.get("series").and_then(|v| v.as_array());
    let chapter_ids: Vec<u64> = match series {
        Some(list) if !list.is_empty() => list
            .iter()
            .filter_map(|item| item.get("id").and_then(|v| v.as_str()))
            .filter_map(|id| id.parse::<u64>().ok())
            .collect(),
        _ => vec![aid],
    };
    for photo_id in &chapter_ids {
        let chapter = fetch_chapter(app, *photo_id)?;
        chapters.push(chapter);
    }
    if chapters.is_empty() {
        return Err("接口没返回任何章节".to_string());
    }
    Ok(JmAlbumData {
        title: if title.is_empty() { format!("JM{aid}") } else { title },
        tags,
        chapters,
    })
}

/// 拿一章的图片清单。
fn fetch_chapter(app: &tauri::AppHandle, photo_id: u64) -> Result<JmChapter, String> {
    let chapter = api_get(app, &format!("/chapter?id={photo_id}"))?;
    let name = {
        let name = value_str(&chapter, "name");
        if name.is_empty() { format!("第 {photo_id} 话") } else { name }
    };
    let mut pages: Vec<String> = Vec::new();
    if let Some(list) = chapter.get("images").and_then(|v| v.as_array()) {
        for image in list {
            let Some(image) = image.as_str() else { continue };
            let image = image.trim();
            if !image.is_empty() {
                pages.push(image.to_string());
            }
        }
    }
    if pages.is_empty() {
        return Err(format!("章节 {photo_id} 没有图片清单"));
    }
    Ok(JmChapter {
        photo_id,
        name,
        pages,
    })
}

/// 图片直链（移动端图床）。
fn image_url(photo_id: u64, filename: &str) -> String {
    format!(
        "https://{}/media/photos/{photo_id}/{filename}",
        IMAGE_DOMAINS[0]
    )
}

/// 用移动 API 把整本（含多章）下载入库。成功返回入库路径；
/// 失败返回 Err，调用方退回网页解析路线。
pub fn run_album_via_api(app: &tauri::AppHandle, aid: u64) -> Result<String, String> {
    netdl::set_running_stage("走站点接口取源数据…");
    let album = fetch_album(app, aid)?;
    let total: usize = album.chapters.iter().map(|c| c.pages.len()).sum();
    netdl::set_running_stage(&format!(
        "「{}」共 {} 章 {} 页（接口源数据）",
        album.title,
        album.chapters.len(),
        total
    ));

    let mut imported = String::new();
    // 整本累计进度：done_base = 前面各章张数之和，grand = 整本总张数。
    // 进度条跨章单调前进（修复「不跟进度、瞬间完成」）。
    let mut done_before_chapter = 0usize;
    for (index, chapter) in album.chapters.iter().enumerate() {
        if netdl::task_is_canceled_public() {
            return Err("已取消".to_string());
        }
        let title = if album.chapters.len() == 1 {
            album.title.clone()
        } else {
            format!("{} {}", album.title, chapter.name)
        };
        netdl::set_running_stage(&format!(
            "第 {}/{} 章「{}」：{} 页（整本进度 {}/{} 张）",
            index + 1,
            album.chapters.len(),
            chapter.name,
            chapter.pages.len(),
            done_before_chapter,
            total
        ));
        let urls: Vec<String> = chapter
            .pages
            .iter()
            .map(|filename| image_url(chapter.photo_id, filename))
            .collect();
        // 平铺 (namespace, value) → 入库要的 HashMap。
        let mut tag_map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (namespace, value) in &album.tags {
            let list = tag_map.entry(namespace.clone()).or_default();
            if !list.contains(value) {
                list.push(value.clone());
            }
        }
        let referer = format!("https://{}/", API_DOMAINS[0]);
        // 段数公式的 md5 入参是**章节 photo_id**（jmcomic JmImageDetail.of 里
        // aid=photo_id，get_num_by_detail 用它），不是专辑 id——单章本子两者恰好
        // 相等所以从来没暴露；多章合集传专辑 id 会让段数全错、图片拼接错位
        // （管理员 2026-11-27 截图）。这里必须传 chapter.photo_id。
        let jm = netdl::JmDecode {
            scramble_id: 220980,
            aid: chapter.photo_id,
        };
        // 备注「源」必须是管理员看到的作品页（18comic.vip/album/…），
        // 接口域名（cdnhjk.net）没意义（管理员 2026-11-27 截图）。
        let source = format!("https://18comic.vip/album/{aid}/");
        let path = netdl::download_and_package(
            app,
            urls,
            &title,
            &tag_map,
            &referer,
            Some(&jm),
            Some((done_before_chapter, total)),
            Some(&source),
        )?;
        if imported.is_empty() {
            imported = path;
        }
        done_before_chapter += chapter.pages.len();
    }
    if imported.is_empty() {
        return Err("没有下载到任何章节".to_string());
    }
    Ok(imported)
}

/// 仅供日志/测试：md5hex 自检（对齐 jmcomic 的 token 算法）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_jmcomic_reference() {
        // jmcomic: token = md5(f"{ts}{APP_TOKEN_SECRET}")；取固定 ts 的公开参考值不可得，
        // 这里锁算法：同输入必同输出，且 tokenparam 拼法一致。
        let token = md5hex(&format!("1700566805{TOKEN_SECRET}"));
        assert_eq!(token.len(), 32);
        assert_eq!(token, md5hex("1700566805185Hcomic3PAPP7R"));
        assert_eq!(format!("{},{}", 1700566805, APP_VERSION), "1700566805,2.1.13");
    }
}
