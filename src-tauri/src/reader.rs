use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use zip::ZipArchive;

use crate::archive::{
    PAGE_DECODE_ALLOC, THUMB_DECODE_ALLOC, decode_image, is_ignored_entry, is_image_name,
};
use crate::fsutil;
use crate::models::{ReaderInfo, ReaderPage};

const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
struct ReaderEntry {
    zip_index: usize,
    name: String,
}

struct Manifest {
    stamp: (i64, u64),
    entries: Vec<ReaderEntry>,
}

fn manifest_cache() -> &'static Mutex<HashMap<PathBuf, Manifest>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Manifest>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_stamp(path: &Path) -> Result<(i64, u64), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "文件不存在，可能已被移动或删除".to_string()
        } else {
            format!("无法读取压缩包信息: {error}")
        }
    })?;
    Ok((fsutil::modified_ms(&metadata), metadata.len()))
}

fn build_entries(archive: &mut ZipArchive<File>) -> Vec<ReaderEntry> {
    let mut entries = Vec::new();

    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }

        let name = entry.name().to_string();
        if is_ignored_entry(&name) || !is_image_name(&name) {
            continue;
        }

        entries.push(ReaderEntry {
            zip_index: index,
            name,
        });
    }

    entries.sort_by(|left, right| natord::compare(&left.name, &right.name));
    entries
}

fn entries_for(path: &Path) -> Result<Vec<ReaderEntry>, String> {
    let stamp = file_stamp(path)?;

    if let Ok(cache) = manifest_cache().lock() {
        if let Some(manifest) = cache.get(path) {
            if manifest.stamp == stamp {
                return Ok(manifest.entries.clone());
            }
        }
    }

    let file = File::open(path).map_err(|error| format!("无法打开压缩包: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("压缩包损坏或格式不支持: {error}"))?;
    let entries = build_entries(&mut archive);

    if entries.is_empty() {
        return Err("压缩包内没有可识别的图片".to_string());
    }

    if let Ok(mut cache) = manifest_cache().lock() {
        cache.insert(
            path.to_path_buf(),
            Manifest {
                stamp,
                entries: entries.clone(),
            },
        );
    }

    Ok(entries)
}

pub fn reader_info(path: &Path, last_page: i64) -> Result<ReaderInfo, String> {
    let entries = entries_for(path)?;
    let page_count = entries.len() as i64;
    Ok(ReaderInfo {
        page_count,
        last_page: last_page.clamp(0, page_count),
        bookmarks: Vec::new(),
    })
}

pub fn read_page(path: &Path, page_index: i64) -> Result<ReaderPage, String> {
    let (name, bytes) = read_page_bytes(path, page_index)?;
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let (bytes, mime) = if matches!(extension.as_str(), "tif" | "tiff") {
        let image = decode_image(&bytes, PAGE_DECODE_ALLOC)?;
        let mut encoded = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut encoded), image::ImageFormat::Png)
            .map_err(|error| format!("页面图片转码失败: {error}"))?;
        (encoded, "image/png")
    } else {
        (bytes, mime_for(&extension))
    };

    Ok(ReaderPage {
        index: page_index,
        name,
        data_url: format!("data:{mime};base64,{}", STANDARD.encode(&bytes)),
    })
}

pub fn read_thumbnail(
    path: &Path,
    page_index: i64,
    max_edge: Option<u32>,
) -> Result<ReaderPage, String> {
    let (name, bytes) = read_page_bytes(path, page_index)?;
    let edge = max_edge.unwrap_or(180).clamp(64, 512);

    let image = decode_image(&bytes, THUMB_DECODE_ALLOC)?;
    let thumbnail = image.thumbnail(edge, edge);

    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 72);
    encoder
        .encode_image(&thumbnail.to_rgb8())
        .map_err(|error| format!("缩略图编码失败: {error}"))?;

    Ok(ReaderPage {
        index: page_index,
        name,
        data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(&encoded)),
    })
}

fn read_page_bytes(path: &Path, page_index: i64) -> Result<(String, Vec<u8>), String> {
    if page_index < 0 {
        return Err("页码不能为负数".to_string());
    }

    let entries = entries_for(path)?;
    let entry = entries
        .get(page_index as usize)
        .ok_or_else(|| "页码超出范围".to_string())?;

    let file = File::open(path).map_err(|error| format!("无法打开压缩包: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("压缩包损坏或格式不支持: {error}"))?;
    let zip_entry = archive
        .by_index(entry.zip_index)
        .map_err(|error| format!("无法读取页面: {error}"))?;

    if zip_entry.size() > MAX_PAGE_BYTES {
        return Err(format!(
            "页面图片超过 {} MB，暂不支持在阅读器中打开",
            MAX_PAGE_BYTES / 1024 / 1024
        ));
    }

    let mut bytes = Vec::new();
    zip_entry
        .take(MAX_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取页面失败: {error}"))?;

    if bytes.len() as u64 > MAX_PAGE_BYTES {
        return Err(format!(
            "页面图片超过 {} MB，暂不支持在阅读器中打开",
            MAX_PAGE_BYTES / 1024 / 1024
        ));
    }

    Ok((entry.name.clone(), bytes))
}

fn mime_for(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}
