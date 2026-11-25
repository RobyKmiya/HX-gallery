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
//!   GM_deleteValue / GM_listValues / GM_addStyle / GM_addElement / GM_openInTab /
//!   GM_notification / GM_download / GM_setClipboard / GM_registerMenuCommand /
//!   GM_xmlhttpRequest`；脚本以 `run.call(window, ...)` 调用，让 UMD 依赖里的
//!   `this` 落在 window 上（Tampermonkey 行为一致）；
//! - `GM_xmlhttpRequest` 的跨域请求走 `window.__xkGmXhr` 队列 + Rust 侧 curl
//!   轮询器（延续本仓「脚本写队列、Rust 轮询读取」的既有模式，页面始终拿不到
//!   IPC，安全边界不变）；文本与二进制（`blob`/`arraybuffer`，base64 回传）都支持；
//! - GM 存值用 localStorage（按脚本 id 命名空间），跨刷新持久；脚本执行错误
//!   记进 `window.__xkGmDiag`（「装了没反应」可查）。
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
    // @require：外部依赖库（jsdelivr 等）。装脚本时拉下来**拼进源码**，
    // 注入引擎只认单文件，不另外发网络请求。拉取失败则整个安装失败，
    // 给出明确提示（本机网络不通时常见）。
    let requires = parse_requires(source);
    let mut bundled = String::new();
    for url in &requires {
        let code = curl_download_text(url)
            .map_err(|error| format!("拉取 @require 依赖失败（{url}）：{error}"))?;
        bundled.push_str(&format!("// ==== @require {url} ====\n{}\n", code));
    }
    if !bundled.is_empty() {
        let (fields, _) = parse_header(source);
        if let Some(name) = fields.iter().find(|(k, _)| k == "name").map(|(_, v)| v.clone()) {
            // 名字带进注释里方便排查
            bundled = format!("// ==== Comic 脚本 {name}：以下为 @require 依赖 ====\n") + &bundled;
        }
        bundled.push_str(source);
        return add_from_source_bundled(dir, &bundled);
    }
    add_from_source_bundled(dir, source)
}

/// 抽出 @require 的 URL 列表（按声明顺序）。
fn parse_requires(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("//")?.trim();
            let rest = rest.strip_prefix("@require")?.trim();
            if rest.is_empty() { None } else { Some(rest.to_string()) }
        })
        .collect()
}

fn add_from_source_bundled(dir: &Path, source: &str) -> Result<ScriptMeta, String> {
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
    // 直接生成可执行函数，而不是把源码存成字符串再 new Function：
    // ① `new Function` 属于 eval，正式包 CSP（script-src 'self'）没有
    //    'unsafe-eval' 时会被拦掉，所有油猴脚本直接失效；
    // ② 顺带省掉 791KB 字符串的二次编译。
    let entries: Vec<String> = scripts
        .iter()
        .map(|(meta, code)| {
            let meta_json =
                serde_json::to_string(meta).unwrap_or_else(|_| "{}".to_string());
            format!(
                "    {{ meta: {meta_json},\n      run: function (GM, unsafeWindow, window) {{\n{code}\n      }} }},"
            )
        })
        .collect();

    format!(
        r#"(function () {{
  if (window.__xkGm) return;
  window.__xkGm = true;
  var SCRIPTS = [
{entries}
  ];

  // 与 Rust 侧 url_matches 等价。历史 bug（2026-11-23 修）：`https?://` 的展开
  // 写成 "http" + pattern.slice(5)，得到的是 "http?://..."，`*://*/*` 这类
  // @match 因此永远匹配不上——Comic Looms 的 `@match *://*/*` 从来没被执行过，
  // 「装了脚本但没有任何按钮」的根因就在这里。Rust 单测只测 Rust 版，
  // JS 版必须靠真机/浏览器侧验证（见 loaderMatchIsEquivalent 的注释）。
  function urlMatches(url, pattern) {{
    if (pattern.indexOf("https?://") === 0) {{
      var tail = pattern.slice(9); // "https?://" 共 9 个字符
      return urlMatches(url, "http://" + tail) || urlMatches(url, "https://" + tail);
    }}
    if (pattern.indexOf("://") === -1 && pattern.indexOf("*") === -1) {{
      return url.indexOf(pattern) === 0;
    }}
    if (pattern.indexOf("*:") === 0) {{
      return urlMatches(url, "https?:" + pattern.slice(2));
    }}
    return globStarMatch(url, pattern);
  }}
  function globStarMatch(url, pattern) {{
    // `*:` 展开后的 `https?:` 前缀在这里收口（Rust 版同样在 glob 里再展开一次）。
    if (pattern.indexOf("https?://") === 0) {{
      var rest = pattern.slice(9); // "https?://" 共 9 个字符
      return globStarMatch(url, "http://" + rest) || globStarMatch(url, "https://" + rest);
    }}
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
      // GM_openInTab：走 window.open，由引擎 on_new_window 接成新标签。
      // 旧实现直接改 location.href——把用户当前页顶掉，和内嵌浏览器语义不符。
      openInTab: function (url, opts) {{
        var target = (url && typeof url === "object") ? url.url : url;
        var active = !(url && typeof url === "object") || url.active !== false;
        try {{
          var opened = window.open(target, "_blank");
          if (!opened) location.href = target;
        }} catch (e) {{ location.href = target; }}
        return active;
      }},
      notification: function (text) {{
        var msg = (text && typeof text === "object") ? text.text : text;
        try {{ console.info("[匣油猴] " + (meta.name || "") + " " + msg); }} catch (e) {{}}
      }},
      // GM_download 两种签名：GM_download(url, name) / GM_download({{url, name, onload, onerror}})
      // 不能直接用 <a download>：跨域地址上 download 属性会被忽略，浏览器当成
      // 普通导航，一点就把当前标签顶到图片地址去了。先取字节、再走 blob: 链接
      // （与页面同源，download 一定生效），顺带也修好图标/大图一键保存。
      download: function (url, name) {{
        var details = (url && typeof url === "object") ? url : {{ url: url, name: name }};
        function fail(error) {{
          try {{ if (details.onerror) details.onerror(error || {{}}); }} catch (e) {{}}
        }}
        if (!details.url) {{ fail({{ error: "GM_download 缺少 url" }}); return; }}
        gm.xmlhttpRequest({{
          method: "GET",
          url: details.url,
          headers: details.headers || {{}},
          responseType: "blob",
          timeout: 600000,
          onload: function (res) {{
            try {{
              var href = window.URL.createObjectURL(res.response);
              var a = document.createElement("a");
              a.href = href;
              if (details.name) a.download = details.name;
              document.body.appendChild(a);
              a.click();
              a.remove();
              setTimeout(function () {{
                try {{ window.URL.revokeObjectURL(href); }} catch (e) {{}}
              }}, 60000);
              try {{ if (details.onload) details.onload({{}}); }} catch (e) {{}}
            }} catch (e) {{ fail(e); }}
          }},
          onerror: function () {{ fail({{ error: "GM_download 取字节失败" }}); }},
        }});
      }},
      // GM_addElement(parent, tag, attrs) / GM_addElement(tag, attrs)
      addElement: function (parent, tag, attrs) {{
        var host = document.body || document.documentElement;
        if (typeof parent === "string") {{
          attrs = tag || {{}};
          tag = parent;
          parent = host;
        }}
        var el = document.createElement(tag);
        var bag = attrs || {{}};
        Object.keys(bag).forEach(function (key) {{
          if (key === "textContent") el.textContent = bag[key];
          else el.setAttribute(key, bag[key]);
        }});
        (parent || host).appendChild(el);
        return el;
      }},
      // GM_setClipboard：优先异步剪贴板，失败退回 execCommand。
      setClipboard: function (text) {{
        var value = String(text == null ? "" : text);
        try {{
          if (navigator.clipboard && navigator.clipboard.writeText) {{
            navigator.clipboard.writeText(value);
            return;
          }}
        }} catch (e) {{}}
        try {{
          var ta = document.createElement("textarea");
          ta.value = value;
          ta.style.position = "fixed";
          ta.style.opacity = "0";
          document.body.appendChild(ta);
          ta.select();
          document.execCommand("copy");
          ta.remove();
        }} catch (e) {{}}
      }},
      // 菜单/资源类 API：内嵌浏览器没有油猴菜单、也没做 @resource 仓库。
      // 给空实现而不是留 undefined——很多脚本启动时无条件调用，undefined 直接崩。
      registerMenuCommand: function () {{ return 0; }},
      unregisterMenuCommand: function () {{}},
      getResourceText: function () {{ return ""; }},
      getResourceUrl: function () {{ return ""; }},
      xmlhttpRequest: function (details) {{
        window.__xkGmXhr = window.__xkGmXhr || {{ next: 1, items: {{}} }};
        var q = window.__xkGmXhr;
        var id = "r" + q.next;
        q.next += 1;
        // responseType 必须原样带给宿主：blob/arraybuffer 要走二进制通道（base64
        // 回传），否则字节被当成 UTF-8 文本过一遍就废了。漫画织机取页面图片、
        // 打 CBZ 全靠这条，回文字它只会拿到一串 U+FFFD（2026-11-23 补）。
        var type = details.responseType || "";
        q.items[id] = {{
          url: details.url,
          method: details.method || "GET",
          headers: details.headers || {{}},
          data: typeof details.data === "string" ? details.data : "",
          responseType: type,
          // 宿主看到这个标记才开进度探针（每 150ms 看一眼落盘字节数）。
          hasProgress: typeof details.onprogress === "function",
          state: "pending",
        }};
        var item = q.items[id];
        var aborted = false;
        // abort() 是否已经回调过（宿主写回 done 后 timer 还会再跑一轮，
        // 不能让 onabort 触发两次）。
        var abortNotified = false;
        var startedAt = Date.now();
        var gotProgress = false;
        var gotHeaders = false;
        // 统一的响应对象：TM 是同一个对象从头用到尾，readyState 一路推进。
        var res = {{
          readyState: 0,
          status: 0,
          statusText: "",
          responseText: "",
          response: "",
          responseType: type,
          responseHeaders: "",
          finalUrl: details.url,
          responseURL: details.url,
          loaded: 0,
          total: 0,
          lengthComputable: false,
          error: "",
        }};
        function ready(state) {{
          res.readyState = state;
          try {{ if (details.onreadystatechange) details.onreadystatechange(res); }} catch (e) {{}}
        }}
        function decode64(b64) {{
          var bin = window.atob(b64);
          var out = new Uint8Array(bin.length);
          for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i) & 255;
          return out;
        }}
        function headerOf(raw, name) {{
          if (!raw) return "";
          var hit = new RegExp("^" + name + ":[ \\t]*(.*)$", "im").exec(raw);
          return hit ? hit[1].trim() : "";
        }}
        // 进度：宿主每 150ms 报一次已收字节数（文件落盘大小），total 取
        // Content-Length，取不到就是 0 → lengthComputable=false。
        item.onProgress = function (loaded, total) {{
          if (aborted) return;
          res.loaded = loaded;
          res.total = total;
          res.lengthComputable = total > 0;
          // 有字节在涨就重置超时基准：大图下载不该被自己的 timeout 掐死。
          startedAt = Date.now();
          if (!gotProgress) {{
            gotProgress = true;
            if (!gotHeaders) {{ gotHeaders = true; ready(2); }}
            ready(3);
          }}
          try {{ if (details.onprogress) details.onprogress(res); }} catch (e) {{}}
        }};
        ready(1);
        setTimeout(function () {{
          if (aborted) return;
          try {{ if (details.onloadstart) details.onloadstart(res); }} catch (e) {{}}
        }}, 0);
        var timer = setInterval(function () {{
          // 注意：这里不能 `if (aborted) return;`。取消后宿主还会写回一次
          // done+aborted，得让这一轮把队列项清掉并回调 onabort；直接 return
          // 会让队列项永久留在页面里（旧实现的泄漏点）。
          var current = q.items[id];
          if (!current) {{ clearInterval(timer); return; }}
          if (current.state !== "done") {{
            // 传统脚本常用 timeout 选项；超时就回 ontimeout，别让它永远等。
            if (details.timeout && Date.now() - startedAt > details.timeout) {{
              clearInterval(timer);
              try {{ delete q.items[id]; }} catch (e) {{}}
              try {{ if (details.ontimeout) details.ontimeout({{ readyState: 4, status: 0 }}); }} catch (e) {{}}
            }}
            return;
          }}
          clearInterval(timer);
          // 被页面取消：宿主已经把 curl 杀了，不回半截数据，按 TM 语义
          // 依次回调 onabort / onloadend 就收场。
          if (current.aborted) {{
            try {{ delete q.items[id]; }} catch (e) {{}}
            res.readyState = 4;
            res.status = 0;
            if (!abortNotified) {{
              abortNotified = true;
              try {{ if (details.onabort) details.onabort(res); }} catch (e) {{}}
            }}
            try {{ if (details.onloadend) details.onloadend(res); }} catch (e) {{}}
            return;
          }}
          var body = current.responseText || "";
          res.status = current.status || 0;
          res.responseHeaders = current.responseHeaders || "";
          res.responseText = body;
          res.response = body;
          res.error = current.errorText || "";
          if (type === "arraybuffer" || type === "blob") {{
            var bytes = current.responseB64 ? decode64(current.responseB64) : new Uint8Array(0);
            res.responseText = "";
            if (type === "blob") {{
              var mime = headerOf(res.responseHeaders, "content-type") ||
                "application/octet-stream";
              try {{ res.response = new Blob([bytes], {{ type: mime }}); }}
              catch (e) {{ res.response = bytes.buffer; }}
            }} else {{
              res.response = bytes.buffer;
            }}
            res.loaded = bytes.length;
            if (!res.total) res.total = bytes.length;
          }} else if (type === "json") {{
            try {{ res.response = JSON.parse(body); }} catch (e) {{ res.response = null; }}
          }}
          // 用完即删：一页图几 MB，堆在页面队列里就是纯泄漏（旧实现从不清理）。
          try {{ delete q.items[id]; }} catch (e) {{}}
          if (!gotHeaders) {{ gotHeaders = true; ready(2); }}
          if (!gotProgress) {{ gotProgress = true; ready(3); }}
          ready(4);
          if (current.error) {{
            // 传输层失败：error 必须是字符串。漫画织机的 onerror 会直接
            // response.error.match(...) 取域名，给 undefined 它自己抛异常、
            // Promise 永不 settle，界面就一直转圈（2026-11-23 补）。
            if (!res.error) res.error = "网络请求失败：" + details.url;
            try {{ if (details.onerror) details.onerror(res); }} catch (e) {{}}
          }} else {{
            try {{ if (details.onload) details.onload(res); }} catch (e) {{}}
          }}
          try {{ if (details.onloadend) details.onloadend(res); }} catch (e) {{}}
        }}, 80);
        // abort()：pending 的直接撤销；已被宿主认领的置 aborted 标记，
        // 宿主取消探针（≤800ms 一次）看到就会 kill curl——以前只停自家
        // 定时器，curl 照样把整个文件下完，磁盘和带宽全白花。
        return {{
          abort: function () {{
            if (abortNotified) return;
            var current = q.items[id];
            if (!current || current.state === "pending") {{
              abortNotified = true;
              aborted = true;
              clearInterval(timer);
              try {{ delete q.items[id]; }} catch (e) {{}}
              res.readyState = 4;
              res.status = 0;
              try {{ if (details.onabort) details.onabort(res); }} catch (e) {{}}
              try {{ if (details.onloadend) details.onloadend(res); }} catch (e) {{}}
              return;
            }}
            aborted = true;
            current.aborted = true;
          }},
        }};
      }},
    }};
    // GM4 风格别名：GM.xmlHttpRequest / GM.getValue / GM.setValue ...
    gm.xmlHttpRequest = gm.xmlhttpRequest;
    return gm;
  }}
  // 传统油猴脚本用全局 GM_xxx（下划线），GM4 脚本用 GM.xxx（点号）。
  // 之前只给了 GM 对象，Comic Looms 这类混用下划线取值的脚本会
  // ReferenceError: GM_getValue is not defined 直接崩掉、UI 不出来（2026-11-23 修）。
  function setGmGlobals(gm) {{
    window.GM = gm;
    window.GM_info = gm.info;
    window.GM_getValue = function (k, d) {{ return gm.getValue(k, d); }};
    window.GM_setValue = function (k, v) {{ return gm.setValue(k, v); }};
    window.GM_deleteValue = function (k) {{ return gm.deleteValue(k); }};
    window.GM_listValues = function () {{ return gm.listValues(); }};
    window.GM_xmlhttpRequest = function (d) {{ return gm.xmlhttpRequest(d); }};
    window.GM_addStyle = function (css) {{ return gm.addStyle(css); }};
    window.GM_addElement = function (a, b, c) {{ return gm.addElement(a, b, c); }};
    window.GM_download = function (u, n) {{ return gm.download(u, n); }};
    window.GM_notification = function (t) {{ return gm.notification(t); }};
    window.GM_openInTab = function (u, o) {{ return gm.openInTab(u, o); }};
    window.GM_setClipboard = function (t, i) {{ return gm.setClipboard(t, i); }};
    window.GM_registerMenuCommand = function (n, f, a) {{
      return gm.registerMenuCommand(n, f, a);
    }};
    window.GM_unregisterMenuCommand = function (id) {{ return gm.unregisterMenuCommand(id); }};
    window.GM_getResourceText = function (n) {{ return gm.getResourceText(n); }};
    window.GM_getResourceURL = function (n) {{ return gm.getResourceUrl(n); }};
    window.unsafeWindow = window;
  }}
  // 诊断用：脚本崩了不能只留一行 console——画匣用户看不到控制台，
  // 「装了脚本但没有任何反应」就成了黑箱。错误同时记进 window.__xkGmDiag，
  // 宿主（或后续诊断脚本）可以 eval 读出来。
  function gmReport(entry, error) {{
    var info = {{
      name: (entry.meta && entry.meta.name) || "",
      id: (entry.meta && entry.meta.id) || "",
      message: String((error && (error.stack || error.message)) || error),
    }};
    window.__xkGmDiag = window.__xkGmDiag || [];
    window.__xkGmDiag.push(info);
    try {{ console.warn("[匣油猴] 脚本执行出错 " + info.name, error); }} catch (e) {{}}
  }}
  function runOne(entry) {{
    try {{
      var gm = makeGm(entry.meta);
      setGmGlobals(gm);
      // 必须 call(window)：脚本源码（含 @require 内联的 UMD 库）里的 `this`
      // 在 Tampermonkey 里就是 window。用 entry.run(...) 调用时 this 会变成
      // 这个 entry 对象，file-saver 这类 `(function(global){{...}})(this)` 的
      // UMD 就把 saveAs 挂到 entry 上，脚本裸引用全局 saveAs 直接
      // ReferenceError 整段崩——Comic Looms 4.16.1 的 UI 不出来就是这个原因
      // （2026-11-23 修）。
      entry.run.call(window, gm, window, window);
      try {{ console.info("[匣油猴] 已执行 " + (entry.meta.name || entry.meta.id)); }} catch (e) {{}}
    }} catch (e) {{
      gmReport(entry, e);
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
    let mut cmd = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd
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

/// 画匣盾脚本源码（编译时嵌入，发布包不依赖外部文件）。
const BUILTIN_SHIELD: &str = include_str!("../../public/huaxia-shield.user.js");

/// 启动时确保「画匣盾」在脚本仓库里。
///
/// store 在 `app_cache_dir`（系统可能清理），所以每次启动都检查；**已存在就不动**，
/// 用户停用或改过都不覆盖。脚本注入在浏览窗口创建时烘焙，装好后重开浏览窗口生效。
pub fn ensure_builtin_scripts(app: &AppHandle) {
    let Some(dir) = store_dir(app) else {
        return;
    };
    let path = dir.join("huaxia-shield.user.js");
    if path.exists() {
        return;
    }
    if let Err(error) = fs::write(&path, BUILTIN_SHIELD) {
        eprintln!("[userscripts] 写入内置画匣盾失败: {error}");
    }
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
        // 2026-11-23：run 必须以 window 为 this 调用，否则 UMD 依赖（file-saver）
        // 把 saveAs 挂到 entry 上，Comic Looms 整段崩、UI 不出来。
        assert!(
            source.contains("entry.run.call(window"),
            "脚本应以 window 为 this 调用"
        );
        assert!(source.contains("GM_setClipboard"), "剪贴板垫片应存在");
        assert!(source.contains("__xkGmDiag"), "脚本错误应可诊断");
        // 2026-11-23：二进制通道。织机取图/打 CBZ 用 blob/arraybuffer，
        // 只回 responseText 的话字节会被当 UTF-8 弄坏，图片永远画不出来。
        assert!(
            source.contains("responseB64") && source.contains("decode64"),
            "blob/arraybuffer 必须走 base64 二进制回传"
        );
        assert!(
            source.contains("responseType: type"),
            "responseType 要带给宿主，否则不知道该不该编二进制"
        );
        assert!(source.contains("new Blob([bytes]"), "blob 响应要还原成 Blob");
        // 2026-11-23：织机下载时靠 onprogress 画进度条、靠 error 字符串报错。
        assert!(
            source.contains("hasProgress"),
            "要告诉宿主有没有 onprogress，别白开探针"
        );
        assert!(
            source.contains("onloadstart") && source.contains("lengthComputable"),
            "onloadstart / 进度事件都要有"
        );
        assert!(
            source.contains("q.onProgress") || source.contains("item.onProgress"),
            "宿主进度回调的入口要挂在队列项上"
        );
        // 2026-11-24：abort 要真取消（宿主 kill curl），不是只停自家定时器。
        assert!(
            source.contains("abortNotified") && source.contains("onabort"),
            "abort 要按 TM 语义回调 onabort，且只回调一次"
        );
        assert!(
            source.contains("current.aborted"),
            "已取消的请求不许把半截数据回给脚本"
        );
        assert!(source.contains("console.log(") && source.contains("hello"), "脚本体应烘焙进加载器");
    }

    #[test]
    fn loader_js_matcher_handles_wildcard_scheme() {
        let source = loader_source(&[]);
        // `*://*/*` 这类 @match 的协议展开必须按 "https?://"（9 个字符）截。
        // 历史上写成 "http" + pattern.slice(5) → 得到 "http?://..."，
        // 于是 `*://*/*` 永远匹配不上：Comic Looms 从未被执行，UI 一个按钮
        // 都没有（2026-11-23 用真实 loader + 真机 Edge 对照实验确认：
        // 旧逻辑 base=false、脚本零执行；修好后 #ehvp-base 正常创建）。
        assert!(
            source.contains("var tail = pattern.slice(9)"),
            "通配协议展开必须按 9 字符截"
        );
        assert!(
            !source.contains("return urlMatches(url, \"http\" + pattern.slice(5))"),
            "旧的错误偏移（slice(5)）不允许回退"
        );
        assert!(source.contains("globStarMatch"), "通配匹配应走 globStarMatch");
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
