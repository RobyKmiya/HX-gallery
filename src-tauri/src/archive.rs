use std::fs::{self};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder};

use crate::container::{self, ContainerEntry};
use crate::fsutil;

const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const COVER_MAX_WIDTH: u32 = 480;
const COVER_MAX_HEIGHT: u32 = 720;

/// 解码后像素边长上限，防止声明超大尺寸的压缩包图片申请 GB 级内存。
const MAX_PIXEL_EDGE: u32 = 12_000;
/// 封面 / 缩略图解码允许的最大分配量。4K 级动画 webp 首帧解码轻松超过 64MB，
/// 太小会让大页集体的封面/缩略图集体失败。
pub(crate) const THUMB_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
/// 阅读器整页解码允许的最大分配量。
pub(crate) const PAGE_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

/// 带资源上限的图片解码。`image::load_from_memory` 内部使用 `Limits::default()`
/// （max_alloc 512 MB、宽高无上限），对高压缩比或声明超大尺寸的图片不安全。
pub(crate) fn decode_image(bytes: &[u8], max_alloc: u64) -> Result<DynamicImage, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("图片格式无法识别: {error}"))?;

    // image 0.25 的 Limits 是 #[non_exhaustive]，没有链式 setter，
    // 只能取默认值后逐字段修改，再用 ImageReader::limits() 应用。
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(max_alloc);
    limits.max_image_width = Some(MAX_PIXEL_EDGE);
    limits.max_image_height = Some(MAX_PIXEL_EDGE);
    reader.limits(limits);

    let decoder = reader
        .into_decoder()
        .map_err(|error| format!("图片无法解码: {error}"))?;
    let (width, height) = decoder.dimensions();
    if width > MAX_PIXEL_EDGE || height > MAX_PIXEL_EDGE {
        return Err(format!(
            "图片尺寸过大（{width}×{height}，上限 {MAX_PIXEL_EDGE}），已跳过"
        ));
    }

    DynamicImage::from_decoder(decoder).map_err(|error| format!("图片无法解码: {error}"))
}

/// 只数「看起来是页」的图片条目，不解压、不解码。
///
/// 导入面板要在用户点确认之前就报出「这本大概多少页」，而 `inspect_archive`
/// 会顺带解码一张封面（几十到几百毫秒）；一次拖 30 本进来就是十几秒的空白。
/// ZIP 只读中央目录（几十毫秒）；RAR 只扫头部；7z solid 包要付一次全量
/// 解码的枚举成本，这是格式本身决定的。
pub fn count_pages(path: &Path) -> Result<i64, String> {
    let entries = container::list_entries(path)?;

    let total = entries
        .iter()
        .filter(|entry| !is_ignored_entry(&entry.name) && is_image_name(&entry.name))
        .count();

    Ok(total as i64)
}

#[derive(Debug, Default)]
pub struct ArchiveInspection {
    pub page_count: i64,
    pub cover_key: Option<String>,
    pub error: Option<String>,
}

pub fn is_supported_archive(path: &Path) -> bool {
    container::kind_for(path).is_some()
}

pub(crate) fn is_image_name(name: &str) -> bool {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "jfif" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff"
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

    let parts: Vec<&str> = normalized.split('/').collect();
    let (dirs, file) = parts.split_at(parts.len() - 1);
    // 点开头的「目录段」（.git/、.trashes/ 等）整棵跳过；但最后一段是文件名，
    // 不在这里按点前缀误杀——「.Amiya._0.webp」这类以点开头的正常动画页要放行，
    // 是否算图片交给 is_image_name 白名单判断。
    if dirs
        .iter()
        .any(|part| part.starts_with('.') && part.len() > 1)
    {
        return true;
    }
    // AppleDouble 资源叉（._xxx.jpg）是 macOS 解压垃圾，文件名层面仍然跳过。
    let file = file[0];
    file.starts_with("._") && file.len() > 2
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
    let entries = match container::list_entries(archive_path) {
        Ok(entries) => entries,
        Err(error) => return inspection_error(error),
    };

    let mut images: Vec<ContainerEntry> = entries
        .into_iter()
        .filter(|entry| !is_ignored_entry(&entry.name) && is_image_name(&entry.name))
        .collect();

    if images.is_empty() {
        return inspection_error("压缩包内没有可识别的图片");
    }

    images.sort_by(|left, right| natord::compare(&left.name, &right.name));

    let cover_position = images
        .iter()
        .position(|entry| {
            Path::new(&entry.name)
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

    // 只判断 exists() 会把中断留下的 0 字节 / 截断文件当成有效封面；
    // 这里至少校验非空，写入失败时临时文件由 save_jpeg 自行清理。
    let cover_ready = || {
        fs::metadata(&cover_path)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
    };

    if !cover_ready() {
        if let Err(create_error) = fs::create_dir_all(covers_dir) {
            error = Some(format!("无法创建封面缓存目录: {create_error}"));
        } else if let Err(write_error) = write_cover(archive_path, &images[cover_position], &cover_path)
        {
            error = Some(write_error);
        }
    }

    ArchiveInspection {
        page_count,
        cover_key: if cover_ready() { Some(cover_key) } else { None },
        error,
    }
}

fn write_cover(path: &Path, entry: &ContainerEntry, destination: &Path) -> Result<(), String> {
    // 大小在列表阶段就已知，超限直接放弃，不把整条读进内存。
    if entry.size > MAX_IMAGE_BYTES {
        return Err("封面图片过大，已跳过".to_string());
    }

    let bytes = container::read_entry(path, entry.raw_index)
        .map_err(|error| format!("读取封面图片失败: {error}"))?;

    let image = decode_image(&bytes, THUMB_DECODE_ALLOC)?;
    let thumbnail = image.thumbnail(COVER_MAX_WIDTH, COVER_MAX_HEIGHT);
    let rgb = thumbnail.to_rgb8();

    save_jpeg(&DynamicImage::ImageRgb8(rgb), destination)
}

fn save_jpeg(image: &DynamicImage, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建封面目录: {error}"))?;
    }

    // 先写同目录临时文件，再 rename 原子替换：避免中断留下截断 JPEG
    // 被下次的 exists() 判断当成有效封面永久使用。
    let name = destination
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "cover".to_string());
    let temp = destination.with_file_name(format!(
        "{name}.{}-{}.tmp",
        std::process::id(),
        fsutil::now_ms()
    ));

    // 显式指定 JPEG 格式（临时文件扩展名是 .tmp，image.save 无法推断格式）。
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), image::ImageFormat::Jpeg)
        .map_err(|error| format!("封面编码失败: {error}"))?;
    fs::write(&temp, &encoded).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("保存封面失败: {error}")
    })?;

    fs::rename(&temp, destination).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("封面写入未完成: {error}")
    })
}

pub fn covers_dir_for(cache_dir: &Path, library_id: &str) -> PathBuf {
    cache_dir.join("covers").join(library_id)
}

/// 封面文件名固定为「内容哈希 + .jpg」。删除封面之前校验形态，
/// 避免被篡改的 `cover_key`（含 `..` 或路径分隔符）拼出缓存目录之外的路径。
pub(crate) fn is_cover_key(key: &str) -> bool {
    let Some(stem) = key.strip_suffix(".jpg") else {
        return false;
    };
    !stem.is_empty() && stem.len() <= 64 && stem.chars().all(|ch| ch.is_ascii_hexdigit())
}
