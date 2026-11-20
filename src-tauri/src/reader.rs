use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::archive::{
    PAGE_DECODE_ALLOC, THUMB_DECODE_ALLOC, decode_image, is_ignored_entry, is_image_name,
};
use crate::container;
use crate::fsutil;
use crate::models::{ReaderInfo, ReaderPage, ReaderThumb};
use crate::pagecache::{ByteCache, Cached};

/// 单页原始字节上限。动画 webp/gif 单文件超过 32MB 很常见（大合集包单页可达
/// 40MB+），32MiB 会把这类页整页拒绝；放宽到 256MiB。超过 PAGE_CACHE_BUDGET 的页
/// 不进内存缓存（pagecache 对超预算单条本来就不落缓存），按需直读，靠 `?v=` 的
/// HTTP 缓存（max-age=3600）避免反复解压。
const MAX_PAGE_BYTES: u64 = 256 * 1024 * 1024;

/// 页面字节缓存预算。动图常见 20–30MB，128MB 够「当前页 + 一两页邻页」共存；
/// 超过预算的单页不缓存，走直读路径。
const PAGE_CACHE_BUDGET: usize = 128 * 1024 * 1024;
const THUMB_CACHE_BUDGET: usize = 24 * 1024 * 1024;
/// 记住多少个「正在读」的压缩包路径，供自定义协议解析。
const MAX_REMEMBERED_ARCHIVES: usize = 32;

#[derive(Clone, Debug)]
struct ReaderEntry {
    /// 条目在容器内的原始序号（container::list_entries 的 raw_index）。
    raw_index: usize,
    name: String,
    /// 原始字节大小，清单阶段即可判超限，不必先整条读入内存。
    size: u64,
}

struct Manifest {
    stamp: (i64, u64),
    entries: Vec<ReaderEntry>,
}

fn manifest_cache() -> &'static Mutex<HashMap<PathBuf, Manifest>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Manifest>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn page_cache() -> &'static Mutex<ByteCache> {
    static CACHE: OnceLock<Mutex<ByteCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ByteCache::new(PAGE_CACHE_BUDGET)))
}

fn thumb_cache() -> &'static Mutex<ByteCache> {
    static CACHE: OnceLock<Mutex<ByteCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ByteCache::new(THUMB_CACHE_BUDGET)))
}

/// 已打开的画集 id → 压缩包路径。
///
/// 自定义协议处理器运行在 webview 的 UI 线程上，不能在那里查数据库
/// （库状态锁可能被长时间导入占着，会直接卡住界面）。这里只放命令层
/// 已经解析过的结果，协议层拿到就是一次哈希查找。
fn archive_paths() -> &'static Mutex<HashMap<i64, PathBuf>> {
    static PATHS: OnceLock<Mutex<HashMap<i64, PathBuf>>> = OnceLock::new();
    PATHS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn remember_archive(archive_id: i64, path: &Path) {
    if let Ok(mut paths) = archive_paths().lock() {
        if paths.len() >= MAX_REMEMBERED_ARCHIVES && !paths.contains_key(&archive_id) {
            paths.clear();
        }
        paths.insert(archive_id, path.to_path_buf());
    }
}

pub fn remembered_archive(archive_id: i64) -> Option<PathBuf> {
    archive_paths()
        .lock()
        .ok()
        .and_then(|paths| paths.get(&archive_id).cloned())
}

pub(crate) fn file_stamp(path: &Path) -> Result<(i64, u64), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "文件不存在，可能已被移动或删除".to_string()
        } else {
            format!("无法读取压缩包信息: {error}")
        }
    })?;
    Ok((fsutil::modified_ms(&metadata), metadata.len()))
}

/// 缓存键带上文件大小与修改时间：体积没有变化就不可能换了内容。
pub(crate) fn cache_key(kind: &str, path: &Path, index: i64, suffix: &str) -> Result<String, String> {
    let (modified, size) = file_stamp(path)?;
    Ok(format!(
        "{kind}|{}|{index}|{suffix}|{modified}|{size}",
        path.to_string_lossy()
    ))
}

fn build_entries(items: Vec<container::ContainerEntry>) -> Vec<ReaderEntry> {
    let mut entries = Vec::new();

    for item in items {
        if is_ignored_entry(&item.name) || !is_image_name(&item.name) {
            continue;
        }

        entries.push(ReaderEntry {
            raw_index: item.raw_index,
            name: item.name,
            size: item.size,
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

    let items = container::list_entries(path)?;
    let entries = build_entries(items);

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

/// 压缩包版本号（修改时间-大小），前端拼进 URL 做缓存键。
pub fn file_version(path: &Path) -> String {
    match file_stamp(path) {
        Ok((modified, size)) => format!("{modified}-{size}"),
        Err(_) => String::new(),
    }
}

/// 把一页准备进后端缓存，返回它的元信息。
///
/// 解码 / TIFF 转码都在调用方（阻塞线程）完成，协议取图时只是内存读取，
/// 因此 webview 的 UI 线程不会因为解码大图而卡顿。
///
/// `pin` 表示这一页正显示在屏幕上：缓存会为它保留位置，不让预取的邻页
/// 把它挤掉（动图单页二三十兆时，预取 3 页就能把当前页顶出 96MB 预算）。
pub fn read_page(path: &Path, page_index: i64, pin: bool) -> Result<ReaderPage, String> {
    let payload = page_payload(path, page_index, pin)?;
    Ok(ReaderPage {
        index: page_index,
        name: payload.name.clone(),
        version: file_version(path),
        bytes: payload.len() as i64,
    })
}

/// 取页面字节：命中缓存直接返回，未命中则解压 + 解码 + 转码后写回缓存。
pub fn page_payload(path: &Path, page_index: i64, pin: bool) -> Result<Cached, String> {
    let key = cache_key("page", path, page_index, "")?;

    if let Ok(mut cache) = page_cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        if pin {
            cache.pin(&key);
        }
        return Ok(hit);
    }

    let payload = extract_page(path, page_index)?;

    if let Ok(mut cache) = page_cache().lock() {
        cache.insert(key.clone(), payload.clone());
        if pin {
            cache.pin(&key);
        }
    }

    Ok(payload)
}

/// 只查缓存，不做任何解压或解码。协议层用它判断是否已预热。
pub fn cached_page(path: &Path, page_index: i64) -> Option<Cached> {
    let key = cache_key("page", path, page_index, "").ok()?;
    page_cache().lock().ok()?.get(&key)
}

fn extract_page(path: &Path, page_index: i64) -> Result<Cached, String> {
    let (name, bytes) = read_page_bytes(path, page_index)?;
    let extension = extension_of(&name);

    if matches!(extension.as_str(), "tif" | "tiff") {
        let image = decode_image(&bytes, PAGE_DECODE_ALLOC)?;
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .map_err(|error| format!("页面图片转码失败: {error}"))?;
        return Ok(Cached::new(name, "image/png", encoded));
    }

    Ok(Cached::new(name, mime_for(&extension), bytes))
}

pub fn read_thumbnail(
    path: &Path,
    page_index: i64,
    max_edge: Option<u32>,
) -> Result<ReaderThumb, String> {
    let edge = max_edge.unwrap_or(180).clamp(64, 512);
    let key = cache_key("thumb", path, page_index, &edge.to_string())?;

    if let Ok(mut cache) = thumb_cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        return Ok(ReaderThumb {
            index: page_index,
            name: hit.name.clone(),
            data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(&*hit.bytes)),
        });
    }

    let (name, bytes) = read_page_bytes(path, page_index)?;
    let image = decode_image(&bytes, THUMB_DECODE_ALLOC)?;
    let thumbnail = image.thumbnail(edge, edge);

    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 72);
    encoder
        .encode_image(&thumbnail.to_rgb8())
        .map_err(|error| format!("缩略图编码失败: {error}"))?;

    if let Ok(mut cache) = thumb_cache().lock() {
        cache.insert(
            key,
            Cached::new(name.clone(), "image/jpeg", encoded.clone()),
        );
    }

    Ok(ReaderThumb {
        index: page_index,
        name,
        data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(&encoded)),
    })
}

/// 取页面原始字节，优先复用页面缓存。
///
/// 动图（动画 webp/gif）一页二三十兆，解码前要先整段读出来；`read_page` 预热时
/// 已经把这份字节放进页面缓存，这里直接复用，避免同一页在压缩包里被解压两遍。
pub(crate) fn page_bytes_for_decode(
    path: &Path,
    page_index: i64,
) -> Result<(String, Vec<u8>), String> {
    let payload = page_payload(path, page_index, false)?;
    Ok((payload.name.clone(), payload.bytes.as_ref().clone()))
}

pub(crate) fn read_page_bytes(path: &Path, page_index: i64) -> Result<(String, Vec<u8>), String> {
    if page_index < 0 {
        return Err("页码不能为负数".to_string());
    }

    let entries = entries_for(path)?;
    let entry = entries
        .get(page_index as usize)
        .ok_or_else(|| "页码超出范围".to_string())?;

    // 大小在清单阶段就已知（ZIP 中央目录 / RAR 头部 / 7z 元数据），超限直接拒绝，
    // 不把整条读进内存。
    if entry.size > MAX_PAGE_BYTES {
        return Err(format!(
            "页面图片超过 {} MB，暂不支持在阅读器中打开",
            MAX_PAGE_BYTES / 1024 / 1024
        ));
    }

    let bytes = container::read_entry(path, entry.raw_index)?;
    if bytes.len() as u64 > MAX_PAGE_BYTES {
        return Err(format!(
            "页面图片超过 {} MB，暂不支持在阅读器中打开",
            MAX_PAGE_BYTES / 1024 / 1024
        ));
    }

    Ok((entry.name.clone(), bytes))
}

fn extension_of(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn mime_for(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}
