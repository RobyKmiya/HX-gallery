//! D2 · 受控 WebView 下载器（M1 最小闭环）
//!
//! 只做 exhentai 作品页的「打开 → 解析首页 → 打包 CBZ → 入库」。
//! 页面侧只解析与取字节，Rust 侧负责落盘与入库。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::utils::config::WebviewUrl;
use tauri::webview::{PageLoadEvent, WebviewWindow};
use tauri::{AppHandle, Manager, State, WebviewWindowBuilder};
use zip::write::SimpleFileOptions;

use crate::models::{ImportOptions, ImportReport, TagDraft};
use crate::{importer, paths, AppState};

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
        (doc.querySelector("#image") || {}).getAttribute?.("src") ||
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct D2FetchReport {
    pub title: String,
    pub source_url: String,
    pub page_url: String,
    pub image_url: String,
    pub cbz_path: String,
    pub import_report: ImportReport,
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

async fn eval_json(window: &WebviewWindow, expression: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<String>();
    window
        .eval_with_callback(expression, move |value| {
            let _ = tx.send(value);
        })
        .map_err(|error| format!("读取页面结果失败：{error}"))?;

    let raw = tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| String::new())
    })
    .await
    .map_err(|error| format!("读取页面结果任务异常：{error}"))?;

    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    Ok(serde_json::from_str::<String>(&raw).unwrap_or(raw))
}

#[tauri::command]
pub async fn d2_fetch_first(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<D2FetchReport, String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "网址格式不正确".to_string())?;
    if parsed.scheme() != "https" {
        return Err("D2 只接受 https 网址".to_string());
    }
    let host = parsed.host_str().unwrap_or("");
    if !host.eq_ignore_ascii_case("exhentai.org")
        && !host.eq_ignore_ascii_case("e-hentai.org")
    {
        return Err("M1 目前只支持 exhentai.org / e-hentai.org".to_string());
    }

    let (root, library_id) = {
        let inner = state.lock()?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (library.root.clone(), library.library_id.clone())
    };

    let label = format!("d2-browser-{}", uuid::Uuid::new_v4());
    let started = Arc::new(AtomicBool::new(false));
    let window = {
        let started = Arc::clone(&started);
        WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::External(parsed))
            .title("画匣 · D2 浏览")
            .inner_size(1120.0, 820.0)
            .initialization_script(D2_INIT_SCRIPT)
            .on_page_load(move |window, payload| {
                if !matches!(payload.event(), PageLoadEvent::Finished) {
                    return;
                }
                // 登录页 / 首页不触发解析；等用户进入作品详情页再抓。
                let href = payload.url().as_str();
                if !href.contains("/g/") && !href.contains("/gallery/") {
                    return;
                }
                if started.swap(true, Ordering::SeqCst) {
                    return;
                }
                let _ = window.eval("window.__d2Parse && window.__d2Parse()");
            })
            .build()
            .map_err(|error| format!("打开浏览窗口失败：{error}"))?
    };

    let mut payload: Option<PagePayload> = None;
    let mut last_error = String::new();
    for _ in 0..120 {
        let json = eval_json(&window, "window.__d2Result ? JSON.stringify(window.__d2Result) : ''").await?;
        if json.trim().is_empty() {
            tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(500)))
                .await
                .ok();
            continue;
        }
        let parsed: PagePayload = serde_json::from_str(&json)
            .map_err(|error| format!("解析页面结果失败：{error}"))?;
        if !parsed.ok {
            last_error = parsed.error.clone().unwrap_or_else(|| "页面解析失败".to_string());
            break;
        }
        payload = Some(parsed);
        break;
    }
    let _ = window.close();

    let payload = payload.ok_or_else(|| {
        if last_error.is_empty() {
            "解析超时：请确认已登录并且页面是作品详情页".to_string()
        } else {
            last_error
        }
    })?;

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
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
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

    let covers_dir = paths::covers_dir(&app, &library_id)?;
    let app_for_import = app.clone();
    let report_path = cbz_path.clone();
    let import_report: ImportReport = tauri::async_runtime::spawn_blocking(move || -> Result<ImportReport, String> {
        let _guard = crate::try_maintenance()?;
        let options = ImportOptions {
            add_to_collection: None,
            preserve_structure: None,
            tags: tag_drafts(&payload.tags),
        };
        Ok(importer::import_blocking(
            &app_for_import,
            &root,
            &covers_dir,
            vec![report_path],
            options,
        ))
    })
    .await
    .map_err(|error| format!("入库任务异常：{error}"))??;

    Ok(D2FetchReport {
        title: payload.title,
        source_url: url,
        page_url: payload.page_url,
        image_url: payload.image_url,
        cbz_path: cbz_path.to_string_lossy().to_string(),
        import_report,
    })
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
