use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use zip::ZipArchive;

const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const COVER_MAX_WIDTH: u32 = 480;
const COVER_MAX_HEIGHT: u32 = 720;

#[derive(Debug, Default)]
pub struct ArchiveInspection {
    pub page_count: i64,
    pub cover_key: Option<String>,
    pub error: Option<String>,
}

pub fn is_supported_archive(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("zip") | Some("cbz")
    )
}

pub(crate) fn is_image_name(name: &str) -> bool {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff"
    )
}

pub(crate) fn is_ignored_entry(name: &str) -> bool {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    if normalized.starts_with("__macosx/")
        || normalized == ".ds_store"
        || normalized.ends_with("/.ds_store")
        || normalized.ends_with("thumbs.db")
    {
        return true;
    }

    normalized
        .split('/')
        .any(|part| part.starts_with('.') && part.len() > 1)
}

fn inspection_error(message: impl Into<String>) -> ArchiveInspection {
    ArchiveInspection {
        page_count: 0,
        cover_key: None,
        error: Some(message.into()),
    }
}

pub fn inspect_archive(
    archive_path: &Path,
    covers_dir: &Path,
    content_hash: &str,
) -> ArchiveInspection {
    let file = match File::open(archive_path) {
        Ok(file) => file,
        Err(error) => return inspection_error(format!("无法打开压缩包: {error}")),
    };

    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            return inspection_error(format!("压缩包损坏或格式不支持: {error}"));
        }
    };

    let mut images: Vec<(usize, String)> = Vec::new();

    for index in 0..archive.len() {
        let entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        if entry.is_dir() {
            continue;
        }

        let name = entry.name().to_string();
        if is_ignored_entry(&name) || !is_image_name(&name) {
            continue;
        }

        images.push((index, name));
    }

    if images.is_empty() {
        return inspection_error("压缩包内没有可识别的图片");
    }

    images.sort_by(|left, right| natord::compare(&left.1, &right.1));

    let cover_position = images
        .iter()
        .position(|(_, name)| {
            Path::new(name)
                .file_stem()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("cover"))
                .unwrap_or(false)
        })
        .unwrap_or(0);

    let page_count = images.len() as i64;
    let cover_key = format!("{content_hash}.jpg");
    let cover_path = covers_dir.join(&cover_key);
    let mut error = None;

    if !cover_path.exists() {
        if let Err(create_error) = fs::create_dir_all(covers_dir) {
            error = Some(format!("无法创建封面缓存目录: {create_error}"));
        } else if let Err(write_error) =
            write_cover(&mut archive, images[cover_position].0, &cover_path)
        {
            let _ = fs::remove_file(&cover_path);
            error = Some(write_error);
        }
    }

    ArchiveInspection {
        page_count,
        cover_key: if cover_path.exists() {
            Some(cover_key)
        } else {
            None
        },
        error,
    }
}

fn write_cover(
    archive: &mut ZipArchive<File>,
    index: usize,
    destination: &Path,
) -> Result<(), String> {
    let entry = archive
        .by_index(index)
        .map_err(|error| format!("无法读取封面图片: {error}"))?;

    if entry.size() > MAX_IMAGE_BYTES {
        return Err("封面图片过大，已跳过".to_string());
    }

    let mut bytes = Vec::new();
    entry
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取封面图片失败: {error}"))?;

    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("封面图片过大，已跳过".to_string());
    }

    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("封面图片无法解码: {error}"))?;
    let thumbnail = image.thumbnail(COVER_MAX_WIDTH, COVER_MAX_HEIGHT);
    let rgb = thumbnail.to_rgb8();

    save_jpeg(&DynamicImage::ImageRgb8(rgb), destination)
}

fn save_jpeg(image: &DynamicImage, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建封面目录: {error}"))?;
    }

    image
        .save(destination)
        .map_err(|error| format!("保存封面失败: {error}"))
}

pub fn covers_dir_for(cache_dir: &Path, library_id: &str) -> PathBuf {
    cache_dir.join("covers").join(library_id)
}
