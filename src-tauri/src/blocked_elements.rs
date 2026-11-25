//! 网页元素屏蔽（管理员 2026-11-26：「网页加元素选择按钮悬浮窗」）。
//!
//! 唯一真源：`%APPDATA%/com.archivegallery.desktop/browser/blocked.json`，
//! 结构 `{ "host": ["css selector", ...] }`。
//! - 页面内由画匣盾脚本的悬浮圆点选元素，经 `__xkReqQ` 的 `blockElement` 动作写入；
//! - 页面加载完成时 `browser.rs` 读它，注入 `selector { display:none !important }`；
//! - 设置→广告拦截 列出、单条删除、清空。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub fn store_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?.join("browser");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("blocked.json"))
}

pub fn load(app: &AppHandle) -> BTreeMap<String, Vec<String>> {
    let Some(path) = store_path(app) else {
        return BTreeMap::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, data: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
    let path = store_path(app).ok_or_else(|| "找不到配置目录".to_string())?;
    let raw = serde_json::to_string_pretty(data).map_err(|e| format!("序列化屏蔽列表失败: {e}"))?;
    std::fs::write(path, raw).map_err(|e| format!("写入屏蔽列表失败: {e}"))
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_start_matches("www.").to_lowercase()
}

pub fn add(app: &AppHandle, host: &str, selector: &str) -> Result<(), String> {
    let host = normalize_host(host);
    let selector = selector.trim();
    if host.is_empty() || selector.is_empty() {
        return Ok(());
    }
    let mut data = load(app);
    let list = data.entry(host).or_default();
    if !list.iter().any(|item| item == selector) {
        list.push(selector.to_string());
    }
    save(app, &data)
}

pub fn remove(app: &AppHandle, host: &str, selector: &str) -> Result<(), String> {
    let host = normalize_host(host);
    let mut data = load(app);
    if let Some(list) = data.get_mut(&host) {
        list.retain(|item| item != selector);
        if list.is_empty() {
            data.remove(&host);
        }
    }
    save(app, &data)
}

pub fn clear(app: &AppHandle) -> Result<(), String> {
    save(app, &BTreeMap::new())
}

/// 取某 host 生效的选择器：精确匹配 + 父域回退（a.b.com 也吃 b.com 的规则）。
pub fn selectors_for(app: &AppHandle, host: &str) -> Vec<String> {
    let data = load(app);
    let host = normalize_host(host);
    let mut out = Vec::new();
    for (key, list) in data.iter() {
        if host == *key || host.ends_with(&format!(".{key}")) {
            out.extend(list.iter().cloned());
        }
    }
    out
}

/// 把选择器拼成注入用的 CSS（每个选择器一条 `display:none !important`）。
pub fn css_for(app: &AppHandle, host: &str) -> String {
    selectors_for(app, host)
        .iter()
        .map(|selector| format!("{selector}{{display:none !important}}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedEntry {
    pub host: String,
    pub selector: String,
    pub count: usize,
}

/// 展平给前端：一行一条（host + selector）。
pub fn list_flat(app: &AppHandle) -> Vec<BlockedEntry> {
    let data = load(app);
    let mut out = Vec::new();
    for (host, list) in data.iter() {
        for selector in list {
            out.push(BlockedEntry {
                host: host.clone(),
                selector: selector.clone(),
                count: list.len(),
            });
        }
    }
    out
}
