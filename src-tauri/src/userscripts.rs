//! 匣浏览器 · 油猴内核。
//!
//! 目标是「生态向 Chrome/Edge 靠齐」的现实落地：WebView2 本体就是 Edge
//! （Chromium）内核，网页兼容性天然齐平；缺的是**脚本生态**——真正的浏览器
//! 扩展在 WebView2 里装不了，所以这里做的是一个 Tampermonkey 兼容子集：
//!
//! - 脚本以 `.user.js` 落盘在脚本仓库（`==UserScript==` 头 + 源码原样）；
//! - 支持头字段：`@name @version @description @match @include @exclude
//!   @run-at document-start/end/idle`（`@grant` 解析后暂不区分，全部给全套 GM）；
//! - 运行时给每个脚本注入 `GM` 垫片：`GM_info / GM_setValue / GM_getValue /
//!   GM_deleteValue / GM_listValues / GM_addStyle / GM_openInTab /
//!   GM_notification / GM_download / GM_xmlhttpRequest`；
//! - `GM_xmlhttpRequest` 的跨域请求走 `window.__xkGmXhr` 队列 + Rust 侧 curl
//!   轮询器（延续本仓「脚本写队列、Rust 轮询读取」的既有模式，页面始终拿不到
//!   IPC，安全边界不变）；文本响应 V1，二进制待续；
//! - GM 存值用 localStorage（按脚本 id 命名空间），跨刷新持久。
//!
//! 已知边界（详见 docs/匣浏览器经验.md）：`@match` 采用简化版通配（整条 URL
//! 匹配，`*` → `.*`，`*:` → `https?:`），不实现 Chrome match-pattern 的 host
//! 段特例；脚本注入在窗口**创建时**烘焙，改动脚本后要重开浏览窗口才生效。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

/// 一个油猴脚本的元数据（从 `==UserScript==` 头解析而来）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScriptMeta {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub matches: Vec<String>,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
    /// document-start / document-end / document-idle。
    #[serde(default)]
    pub run_at: String,
    pub enabled: bool,
}

/// 脚本仓库根目录。
pub fn store_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_cache_dir().ok()?.join("browser").join("userscripts");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

fn read_state(dir: &Path) -> HashMap<String, bool> {
    fs::read_to_string(state_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, state: &HashMap<String, bool>) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| format!("序列化脚本状态失败: {e}"))?;
    fs::write(state_path(dir), raw).map_err(|e| format!("写脚本状态失败: {e}"))
}

/// 从脚本源码解析 `==UserScript==` 头，返回（头字段表, 脚本体）。
/// 没有头块的脚本返回空表 + 原文（当作无规则全站脚本）。
fn parse_header(source: &str) -> (Vec<(String, String)>, String) {
    const OPEN: &str = "==UserScript==";
    const CLOSE: &str = "==/UserScript==";
    let open_index = match source.find(OPEN) {
        Some(index) => index + OPEN.len(),
        None => return (Vec::new(), source.to_string()),
    };
    let close_index = match source[open_index..].find(CLOSE) {
        Some(index) => open_index + index,
        None => return (Vec::new(), source.to_string()),
    };

    let mut fields = Vec::new();
    for line in source[open_index..close_index].lines() {
        let trimmed = line.trim();
        let body = match trimmed.strip_prefix("//") {
            Some(body) => body.trim(),
            None => continue,
        };
        let body = match body.strip_prefix("@") {
            Some(body) => body.trim(),
            None => continue,
        };
        let (key, value) = match body.split_once(char::is_whitespace) {
            Some((key, rest)) => (key, rest.trim()),
            None => (body, ""),
        };
        fields.push((key.to_string(), value.to_string()));
    }
    (fields, source.to_string())
}

/// 由头字段表构建元数据（id/由调用方给，enabled 由仓库状态给）。
fn meta_from_fields(fields: &[(String, String)], id: String, enabled: bool) -> ScriptMeta {
    let mut meta = ScriptMeta {
        id,
        name: String::new(),
        version: String::new(),
        description: String::new(),
        matches: Vec::new(),
        includes: Vec::new(),
        excludes: Vec::new(),
        run_at: "document-idle".to_string(),
        enabled,
    };
    for (key, value) in fields {
        match key.as_str() {
            "name" => meta.name = value.clone(),
            "version" => meta.version = value.clone(),
            "description" => meta.description = value.clone(),
            "match" => meta.matches.push(value.clone()),
            "include" => meta.includes.push(value.clone()),
            "exclude" => meta.excludes.push(value.clone()),
            "run-at" => meta.run_at = value.trim().to_string(),
            // @grant/@namespace/@icon 等暂不参与行为，保留未来扩展空间
            _ => {}
        }
    }
    if meta.name.is_empty() {
        meta.name = format!("脚本-{}", &meta.id[..8.min(meta.id.len())]);
    }
    meta
}

/// 列出仓库里的全部脚本（禁用的也列出，由前端决定展示方式）。
pub fn list_scripts(dir: &Path) -> Vec<ScriptMeta> {
    let state = read_state(dir);
    let mut metas = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return metas,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("js") {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".user.js"))
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) => continue,
        };
        let (fields, _) = parse_header(&source);
        let enabled = *state.get(&id).unwrap_or(&true);
        metas.push(meta_from_fields(&fields, id, enabled));
    }
    metas.sort_by(|left, right| left.name.cmp(&right.name));
    metas
}

/// 把一段脚本源码存入仓库（解析头 + 落盘 + 默认启用）。返回它的元数据。
pub fn add_from_source(dir: &Path, source: &str) -> Result<ScriptMeta, String> {
    if source.trim().is_empty() {
        return Err("脚本内容为空".to_string());
    }
    let id = Uuid::new_v4().to_string();
    let (fields, _) = parse_header(source);
    let meta = meta_from_fields(&fields, id.clone(), true);
    let path = dir.join(format!("{id}.user.js"));
    fs::write(&path, source).map_err(|e| format!("写脚本文件失败: {e}"))?;

    let mut state = read_state(dir);
    state.insert(id, true);
    write_state(dir, &state)?;
    Ok(meta)
}

/// 删除脚本（文件 + 状态记录）。
pub fn remove_script(dir: &Path, id: &str) -> Result<(), String> {
    // id 白名单校验：只允许 uuid 形态，防止路径穿越。
    if Uuid::parse_str(id).is_err() {
        return Err("脚本 id 不合法".to_string());
    }
    let removed = fs::remove_file(dir.join(format!("{id}.user.js")))
        .map_err(|e| format!("删除脚本文件失败: {e}"));
    let mut state = read_state(dir);
    state.remove(id);
    let _ = write_state(dir, &state);
    removed
}

/// 启用/停用脚本（停用的脚本不参与注入，文件保留）。
pub fn set_enabled(dir: &Path, id: &str, enabled: bool) -> Result<(), String> {
    if Uuid::parse_str(id).is_err() {
        return Err("脚本 id 不合法".to_string());
    }
    if !dir.join(format!("{id}.user.js")).exists() {
        return Err("脚本不存在".to_string());
    }
    let mut state = read_state(dir);
    state.insert(id.to_string(), enabled);
    write_state(dir, &state)
}

/// 简化版 URL 匹配（纯字面量 + `*` 通配，不走正则）：
/// - `https?:` 前缀展开成 http/https 各试一次（`*://` 展开的产物）；
/// - 无协议头且无 `*` 的 @include 按前缀匹配（Tampermonkey 常见写法）；
/// - 其余按 `*` 分段做字面量通配（首段前缀、尾段后缀、中间按序出现）。
fn url_matches(url: &str, pattern: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("https?://") {
        return url_matches(url, &format!("http://{rest}"))
            || url_matches(url, &format!("https://{rest}"));
    }
    if !pattern.contains("://") && !pattern.contains('*') {
        return url.starts_with(pattern);
    }
    let pattern = if let Some(rest) = pattern.strip_prefix("*:") {
        format!("https?:{rest}")
    } else {
        pattern.to_string()
    };
    glob_star_match(&pattern, url)
}

/// 纯字面量的 `*` 通配匹配（非 `*` 字符一律按字面量比对）。
fn glob_star_match(pattern: &str, url: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("https?://") {
        return glob_star_match(&format!("http://{rest}"), url)
            || glob_star_match(&format!("https://{rest}"), url);
    }
    let pieces: Vec<&str> = pattern.split('*').collect();
    let mut cursor = 0usize;
    for (i, part) in pieces.iter().copied().enumerate() {
        let last = i == pieces.len() - 1;
        if i == 0 {
            if !url.starts_with(part) {
                return false;
            }
            cursor = part.len();
        } else if last {
            if url.len() < cursor + part.len() {
                return false;
            }
            if &url[url.len() - part.len()..] != part {
                return false;
            }
            cursor = url.len();
        } else {
            match url[cursor..].find(part) {
                Some(found) => cursor += found + part.len(),
                None => return false,
            }
        }
        if cursor > url.len() {
            return false;
        }
    }
    true
}

/// 脚本是否作用于这个 URL：@exclude 一票否决；
/// 有 @match 用 @match，否则用 @include；两者都没有 = 全站生效。
pub fn match_url(meta: &ScriptMeta, url: &str) -> bool {
    for pattern in &meta.excludes {
        if url_matches(url, pattern) {
            return false;
        }
    }
    let patterns = if !meta.matches.is_empty() {
        &meta.matches
    } else {
        &meta.includes
    };
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|pattern| url_matches(url, pattern))
}

/// 生成注入到浏览器窗口的加载器脚本（窗口创建时烘焙全部启用脚本的源码）。
pub fn loader_source(scripts: &[(ScriptMeta, String)]) -> String {
    let entries: Vec<String> = scripts
        .iter()
        .map(|(meta, code)| {
            let meta_json =
                serde_json::to_string(meta).unwrap_or_else(|_| "{}".to_string());
            let code_json = serde_json::to_string(code).unwrap_or_default();
            format!("    {{ meta: {meta_json}, code: {code_json} }},")
        })
        .collect();

    format!(
        r#"(function () {{
  if (window.__xkGm) return;
  window.__xkGm = true;
  var SCRIPTS = [
{entries}
  ];

  function urlMatches(url, pattern) {{
    if (pattern.indexOf("https?://") === 0) {{
      return urlMatches(url, "http" + pattern.slice(5)) || urlMatches(url, "https" + pattern.slice(5));
    }}
    if (pattern.indexOf("://") === -1 && pattern.indexOf("*") === -1) {{
      return url.indexOf(pattern) === 0;
    }}
    if (pattern.indexOf("*:") === 0) pattern = "https?:" + pattern.slice(2);
    try {{
      var pieces = pattern.split("*");
      var cursor = 0;
      for (var i = 0; i < pieces.length; i++) {{
        var part = pieces[i];
        var last = i === pieces.length - 1;
        if (i === 0) {{
          if (url.indexOf(part) !== 0) return false;
          cursor = part.length;
        }} else if (last) {{
          if (url.length < cursor + part.length) return false;
          if (url.slice(url.length - part.length) !== part) return false;
          cursor = url.length;
        }} else {{
          var found = url.indexOf(part, cursor);
          if (found === -1) return false;
          cursor = found + part.length;
        }}
      }}
      return true;
    }} catch (e) {{
      return false;
    }}
  }}
  function matchUrl(url, meta) {{
    var i;
    for (i = 0; i < (meta.excludes || []).length; i++) {{
      if (urlMatches(url, meta.excludes[i])) return false;
    }}
    var list = (meta.matches && meta.matches.length) ? meta.matches : (meta.includes || []);
    if (!list.length) return true;
    for (i = 0; i < list.length; i++) if (urlMatches(url, list[i])) return true;
    return false;
  }}

  function storeKey(id) {{ return "__xkGmStore_" + id; }}
  function gmStore(id) {{
    try {{ return JSON.parse(localStorage.getItem(storeKey(id)) || "{{}}"); }} catch (e) {{ return {{}}; }}
  }}
  function gmSave(id, data) {{
    try {{ localStorage.setItem(storeKey(id), JSON.stringify(data)); }} catch (e) {{}}
  }}
  function makeGm(meta) {{
    var gm = {{
      info: {{ script: {{ name: meta.name, version: meta.version, description: meta.description }}, uuid: meta.id }},
      addStyle: function (css) {{
        var s = document.createElement("style");
        s.textContent = css;
        (document.head || document.documentElement).appendChild(s);
      }},
      setValue: function (k, v) {{ var d = gmStore(meta.id); d[k] = v; gmSave(meta.id, d); }},
      getValue: function (k, def) {{ var d = gmStore(meta.id); return (k in d) ? d[k] : def; }},
      deleteValue: function (k) {{ var d = gmStore(meta.id); delete d[k]; gmSave(meta.id, d); }},
      listValues: function () {{ return Object.keys(gmStore(meta.id)); }},
      openInTab: function (url) {{ location.href = url; }},
      notification: function (text) {{
        try {{ console.info("[匣油猴] " + (meta.name || "") + " " + text); }} catch (e) {{}}
      }},
      download: function (url, name) {{
        var a = document.createElement("a");
        a.href = url;
        if (name) a.download = name;
        document.body.appendChild(a);
        a.click();
        a.remove();
      }},
      xmlhttpRequest: function (details) {{
        window.__xkGmXhr = window.__xkGmXhr || {{ next: 1, items: {{}} }};
        var q = window.__xkGmXhr;
        var id = "r" + q.next;
        q.next += 1;
        q.items[id] = {{
          url: details.url,
          method: details.method || "GET",
          headers: details.headers || {{}},
          data: details.data || "",
          state: "pending",
        }};
        var timer = setInterval(function () {{
          var item = q.items[id];
          if (!item || item.state !== "done") return;
          clearInterval(timer);
          var res = {{
            readyState: 4,
            status: item.status || 0,
            responseText: item.responseText || "",
            responseHeaders: item.responseHeaders || "",
          }};
          if (item.error) {{ if (details.onerror) details.onerror(res); }}
          else if (details.onload) details.onload(res);
        }}, 80);
        return {{ abort: function () {{ clearInterval(timer); delete q.items[id]; }} }};
      }},
    }};
    return gm;
  }}
  function runOne(entry) {{
    try {{
      var fn = new Function("GM", "unsafeWindow", "window", entry.code);
      fn(makeGm(entry.meta), window, window);
    }} catch (e) {{
      try {{ console.warn("[匣油猴] 脚本执行出错 " + (entry.meta.name || entry.meta.id), e); }} catch (e2) {{}}
    }}
  }}
  var start = [];
  var end = [];
  var href = location.href;
  for (var i = 0; i < SCRIPTS.length; i++) {{
    var entry = SCRIPTS[i];
    if (!matchUrl(href, entry.meta)) continue;
    if ((entry.meta.runAt || "document-idle") === "document-start") start.push(entry);
    else end.push(entry);
  }}
  function runList(list) {{ for (var j = 0; j < list.length; j++) runOne(list[j]); }}
  runList(start);
  if (document.readyState === "loading") {{
    document.addEventListener("DOMContentLoaded", function () {{ runList(end); }});
  }} else {{
    runList(end);
  }}
}})();
"#,
        entries = entries.join("
")
    )
}

// ── Tauri 命令（D2Panel 的油猴管理用）──
//
// 注意：这些命令只管理**脚本仓库**；脚本的注入发生在浏览窗口创建时
// （browser::open 烘焙加载器），所以改完脚本要重开浏览窗口才生效。

#[tauri::command]
pub fn gm_list_scripts(app: AppHandle) -> Result<Vec<ScriptMeta>, String> {
    let dir = store_dir(&app).ok_or("脚本仓库目录不可用")?;
    Ok(list_scripts(&dir))
}

#[tauri::command]
pub fn gm_add_script_from_path(app: AppHandle, path: String) -> Result<ScriptMeta, String> {
    let dir = store_dir(&app).ok_or("脚本仓库目录不可用")?;
    let source =
        fs::read_to_string(&path).map_err(|e| format!("读取脚本文件失败: {e}"))?;
    add_from_source(&dir, &source)
}

#[tauri::command]
pub fn gm_add_script_from_url(app: AppHandle, url: String) -> Result<ScriptMeta, String> {
    let dir = store_dir(&app).ok_or("脚本仓库目录不可用")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("脚本地址必须是 http(s) 链接".to_string());
    }
    let source = curl_download_text(&url)?;
    add_from_source(&dir, &source)
}

#[tauri::command]
pub fn gm_remove_script(app: AppHandle, id: String) -> Result<(), String> {
    let dir = store_dir(&app).ok_or("脚本仓库目录不可用")?;
    remove_script(&dir, &id)
}

#[tauri::command]
pub fn gm_set_script_enabled(app: AppHandle, id: String, enabled: bool) -> Result<(), String> {
    let dir = store_dir(&app).ok_or("脚本仓库目录不可用")?;
    set_enabled(&dir, &id, enabled)
}

/// 用 Windows 自带 curl 拉一段文本（脚本安装源）。
fn curl_download_text(url: &str) -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .arg("-sL")
        .arg("--max-time")
        .arg("60")
        .arg(url)
        .output()
        .map_err(|e| format!("下载脚本失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "下载脚本失败（curl 退出码 {:?}）",
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"// ==UserScript==
// @name         阅读增强
// @version      1.2
// @description  测试脚本
// @match        https://exhentai.org/*
// @match        *://e-hentai.org/s/*
// @exclude      https://exhentai.org/full*
// @run-at       document-start
// @grant        GM_xmlhttpRequest
// ==/UserScript==
console.log("hello");
"#;

    #[test]
    fn parses_header_fields() {
        let (fields, body) = parse_header(SAMPLE);
        assert!(fields.iter().any(|(k, v)| k == "name" && v == "阅读增强"));
        assert!(fields.iter().any(|(k, v)| k == "run-at" && v == "document-start"));
        assert!(body.contains("console.log"), "脚本体应原样保留");
        let meta = meta_from_fields(&fields, "test-id".to_string(), true);
        assert_eq!(meta.name, "阅读增强");
        assert_eq!(meta.matches.len(), 2);
        assert_eq!(meta.excludes.len(), 1);
        assert_eq!(meta.run_at, "document-start");
    }

    #[test]
    fn headerless_script_is_site_wide() {
        let (fields, body) = parse_header("alert(1);");
        assert!(fields.is_empty());
        assert!(body.contains("alert"));
        let meta = meta_from_fields(&fields, "x".to_string(), true);
        assert!(match_url(&meta, "https://anything.example/页"), "无规则脚本全站生效");
    }

    #[test]
    fn match_and_exclude_decide_scope() {
        let (fields, _) = parse_header(SAMPLE);
        let meta = meta_from_fields(&fields, "test".to_string(), true);
        assert!(match_url(&meta, "https://exhentai.org/g/1/2/"));
        assert!(match_url(&meta, "https://e-hentai.org/s/abc/def"));
        assert!(match_url(&meta, "http://e-hentai.org/s/abc/def"), "*: 前缀含 http");
        assert!(!match_url(&meta, "https://exhentai.org/full/alb"), "@exclude 一票否决");
        assert!(!match_url(&meta, "https://nhentai.net/"), "不在 @match 里");
    }

    #[test]
    fn include_prefix_match_without_wildcard() {
        let meta = ScriptMeta {
            id: "x".to_string(),
            includes: vec!["https://e-hentai.org".to_string()],
            ..Default::default()
        };
        assert!(match_url(&meta, "https://e-hentai.org/g/1/"));
        assert!(!match_url(&meta, "https://exhentai.org/"));
    }

    #[test]
    fn loader_contains_scripts_and_gm_shim() {
        let (fields, body) = parse_header(SAMPLE);
        let meta = meta_from_fields(&fields, "abc-123".to_string(), true);
        let source = loader_source(&[(meta, body)]);
        assert!(source.contains("__xkGm"));
        assert!(source.contains("阅读增强"), "脚本名应烘焙进加载器");
        assert!(source.contains("GM_xmlhttpRequest") || source.contains("xmlhttpRequest"));
        assert!(source.contains("__xkGmXhr"), "跨域 XHR 队列应存在");
        assert!(source.contains("console.log(") && source.contains("hello"), "脚本体应烘焙进加载器");
    }

    #[test]
    fn store_add_list_toggle_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!("xk-gm-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let meta = add_from_source(&dir, SAMPLE).unwrap();
        assert_eq!(meta.name, "阅读增强");

        let listed = list_scripts(&dir);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].enabled);

        set_enabled(&dir, &meta.id, false).unwrap();
        let listed = list_scripts(&dir);
        assert!(!listed[0].enabled, "停用后列表里应标记为禁用");

        remove_script(&dir, &meta.id).unwrap();
        assert!(list_scripts(&dir).is_empty());
        assert!(remove_script(&dir, &meta.id).is_err(), "重复删除报错");
        assert!(remove_script(&dir, "../../etc/passwd").is_err(), "非法 id 拒绝");
        let _ = fs::remove_dir_all(&dir);
    }
}
