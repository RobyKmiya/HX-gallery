use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::archive;

pub fn covers_dir(app: &AppHandle, library_id: &str) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法获取缓存目录: {error}"))?;
    let covers_dir = archive::covers_dir_for(&cache_dir, library_id);
    fs::create_dir_all(&covers_dir).map_err(|error| format!("无法创建封面缓存目录: {error}"))?;
    Ok(covers_dir)
}
