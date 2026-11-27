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

/// Adobe CMYK JPEG 嗅探：SOF 组件数 == 4 且带 APP14 Adobe 标记。
///
/// 这类 JPEG 按 Adobe 惯例**存储值是反相的**（libjpeg 系解码器读的时候会自动
/// 反相），而 image 0.25 的 JPEG 解码器 zune-jpeg **不做这个反相** → 解出来
/// 整图品红罩（管理员 2026-11-28：18comic/exhentai 封面色偏的根因）。
fn jpeg_adobe_cmyk(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return false;
    }
    let mut components4 = false;
    let mut adobe_cmyk_family = false;
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let seglen = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        // SOF0~SOF15（排除 DHT/DAC/JPG/RST 扩展段）：第 9 字节是组件数。
        if (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
            && i + 9 < bytes.len()
        {
            components4 = bytes[i + 9] == 4;
        }
        if marker == 0xEE && seglen >= 14 && bytes.get(i + 4..i + 9) == Some(b"Adobe") {
            // Adobe 段尾字节是 transform：0=CMYK、1=YCbCr、2=YCCK。
            // 注意 APP14 通常在 SOF **之前**出现，不能在这里提前 return。
            let transform = bytes[i + 2 + seglen - 1];
            adobe_cmyk_family = matches!(transform, 0 | 2);
        }
        if marker == 0xDA {
            break; // SOS：扫描结束，综合判定
        }
        i += 2 + seglen;
    }
    components4 && adobe_cmyk_family
}

/// Adobe CMYK JPEG 的正确解码：让 zune 输出 CMYK 四通道，按 Adobe 惯例反相
/// 存储值，再用标准公式（RGB = (1-C)(1-K) 等）转 RGB。
fn decode_cmyk_jpeg(bytes: &[u8]) -> Result<DynamicImage, String> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        std::io::Cursor::new(bytes),
        DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::CMYK),
    );
    let pixels = decoder
        .decode()
        .map_err(|error| format!("CMYK JPEG 解码失败: {error}"))?;
    let info = decoder.info().ok_or("CMYK JPEG 缺少图片信息")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if pixels.len() < (width as usize) * (height as usize) * 4 {
        return Err("CMYK JPEG 像素数据不足".to_string());
    }

    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for px in pixels.chunks_exact(4) {
        // Adobe 惯例：存储值 = 255 - 真实值，先反相回真实 CMYK。
        let c = 255 - px[0];
        let m = 255 - px[1];
        let y = 255 - px[2];
        let k = 255 - px[3] as u16;
        // RGB = (1 - C) × (1 - K)，与 zune/libjpeg 的 blinn 内积同式。
        rgb.push((((255 - c) as u16 * (255 - k as u16)) / 255) as u8);
        rgb.push((((255 - m) as u16 * (255 - k as u16)) / 255) as u8);
        rgb.push((((255 - y) as u16 * (255 - k as u16)) / 255) as u8);
    }
    image::RgbImage::from_raw(width, height, rgb)
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| "CMYK JPEG 像素尺寸不匹配".to_string())
}

/// 带资源上限的图片解码。`image::load_from_memory` 内部使用 `Limits::default()`
/// （max_alloc 512 MB、宽高无上限），对高压缩比或声明超大尺寸的图片不安全。
pub(crate) fn decode_image(bytes: &[u8], max_alloc: u64) -> Result<DynamicImage, String> {
    // Adobe CMYK JPEG 特殊路径：zune-jpeg（image 0.25 的 JPEG 解码器）不处理
    // Adobe 存储反相 → 解出整图品红罩（管理员 2026-11-28 封面色偏）。
    if jpeg_adobe_cmyk(bytes) {
        return decode_cmyk_jpeg(bytes);
    }

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
    inspect_archive_with_password(archive_path, covers_dir, content_hash, None)
}

/// 带解压密码的检查（加密 ZIP 抽封面时用）。
pub fn inspect_archive_with_password(
    archive_path: &Path,
    covers_dir: &Path,
    content_hash: &str,
    password: Option<&str>,
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
        } else if let Err(write_error) = write_cover(
            archive_path,
            &images[cover_position],
            &cover_path,
            password,
        ) {
            error = Some(write_error);
        }
    }

    ArchiveInspection {
        page_count,
        cover_key: if cover_ready() { Some(cover_key) } else { None },
        error,
    }
}

fn write_cover(
    path: &Path,
    entry: &ContainerEntry,
    destination: &Path,
    password: Option<&str>,
) -> Result<(), String> {
    // 大小在列表阶段就已知，超限直接放弃，不把整条读进内存。
    if entry.size > MAX_IMAGE_BYTES {
        return Err("封面图片过大，已跳过".to_string());
    }

    let bytes = container::read_entry_with_password(path, entry.raw_index, password)
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

#[cfg(test)]
mod cmyk_tests {
    use super::*;

    /// 最小合法 JPEG 骨架：SOI + 给定 SOF/APP14/SOS 段 + EOI。
    fn minimal_jpeg(sof_marker: u8, components: u8, adobe_transform: Option<u8>) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        if let Some(t) = adobe_transform {
            // APP14："Adobe" 5 字节 + version 2 + flags0/flags1 4 + transform 1 = 12 载荷
            let mut payload = b"Adobe".to_vec();
            payload.extend_from_slice(&[0x64, 0x00, 0x00, 0x00, 0x00, 0x00, t]);
            out.extend_from_slice(&[0xFF, 0xEE]);
            out.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
            out.extend_from_slice(&payload);
        }
        // SOF0：precision 1 + 高 2 + 宽 2 + 组件数 1 + 每组件 3 字节
        let sof_len = 8 + 3 * components as u16;
        out.extend_from_slice(&[0xFF, sof_marker]);
        out.extend_from_slice(&sof_len.to_be_bytes());
        out.push(8);
        out.extend_from_slice(&[0x00, 0x10, 0x00, 0x10, components]);
        for i in 0..components {
            out.extend_from_slice(&[i + 1, 0x11, 0]);
        }
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]); // SOS 头（截断无妨）
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn detects_adobe_cmyk_and_ycck() {
        assert!(jpeg_adobe_cmyk(&minimal_jpeg(0xC0, 4, Some(0))));
        assert!(jpeg_adobe_cmyk(&minimal_jpeg(0xC2, 4, Some(2))));
        assert!(!jpeg_adobe_cmyk(&minimal_jpeg(0xC0, 3, Some(1))));
        assert!(!jpeg_adobe_cmyk(&minimal_jpeg(0xC0, 4, None)), "无 Adobe 段不反相");
        assert!(!jpeg_adobe_cmyk(&[0x89, 0x50, 0x4E, 0x47]), "PNG 不误判");
    }
}
