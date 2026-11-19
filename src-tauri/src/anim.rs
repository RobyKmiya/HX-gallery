//! 动图（gif / 动画 webp）的帧级元数据与帧解码。
//!
//! 协议层跑在 webview 的 UI 线程上不能做重活，所以 meta 与帧 PNG 都在命令层
//! （spawn_blocking）算好放进缓存，`gallery://` 协议只负责从缓存取字节。
//!
//! - gif：走 image crate 的 `AnimationDecoder`（逐帧已合成到画布）；
//! - 动画 webp：走 image-webp 的动画 API（is_animated / num_frames /
//!   loop_duration / read_frame），帧序列内部已处理混合与还原。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, DynamicImage, Frame, ImageDecoder, ImageFormat};

use crate::pagecache::{ByteCache, Cached};
use crate::reader;

/// 帧字节缓存预算。4K 页的单帧 PNG 约 3–5MB，160MB 够拖条来回擦洗。
const FRAME_CACHE_BUDGET: usize = 160 * 1024 * 1024;
/// meta 缓存条数上限（条目很小，超了整体清空）。
const META_CACHE_LIMIT: usize = 256;

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct AnimMeta {
    pub animated: bool,
    pub frames: u32,
    /// 所有帧时长之和（毫秒）。播放模式用它决定「播完翻页」的时刻。
    pub total_duration_ms: u64,
    pub width: u32,
    pub height: u32,
}

impl AnimMeta {
    fn still(width: u32, height: u32) -> Self {
        Self {
            animated: false,
            frames: 0,
            total_duration_ms: 0,
            width,
            height,
        }
    }
}

fn frame_cache() -> &'static Mutex<ByteCache> {
    static CACHE: OnceLock<Mutex<ByteCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ByteCache::new(FRAME_CACHE_BUDGET)))
}

fn meta_cache() -> &'static Mutex<HashMap<(PathBuf, i64, (i64, u64)), AnimMeta>> {
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, i64, (i64, u64)), AnimMeta>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 命令层入口：取某页的动画元数据（非动图页返回 animated=false）。
pub(crate) fn anim_meta(path: &Path, page_index: i64) -> Result<AnimMeta, String> {
    let stamp = reader::file_stamp(path)?;
    let key = (path.to_path_buf(), page_index, stamp);
    if let Ok(cache) = meta_cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        return Ok(hit.clone());
    }

    let (name, bytes) = reader::read_page_bytes(path, page_index)?;
    let meta = match extension_of(&name).as_str() {
        "gif" => gif_meta(&bytes)?,
        "webp" => webp_meta(&bytes)?,
        _ => AnimMeta::still(0, 0),
    };

    if let Ok(mut cache) = meta_cache().lock() {
        if cache.len() >= META_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, meta.clone());
    }
    Ok(meta)
}

/// 命令层入口：把第 `frame_index` 帧解码成 PNG 并放进缓存。
pub(crate) fn prepare_frame(path: &Path, page_index: i64, frame_index: u32) -> Result<Cached, String> {
    let key = reader::cache_key("frame", path, page_index, &frame_index.to_string())?;
    if let Ok(mut cache) = frame_cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        return Ok(hit);
    }

    let (name, bytes) = reader::read_page_bytes(path, page_index)?;
    let png = match extension_of(&name).as_str() {
        "gif" => gif_frame_png(&bytes, frame_index)?,
        "webp" => webp_frame_png(&bytes, frame_index)?,
        _ => return Err("该页不是动图格式".to_string()),
    };
    let cached = Cached::new(format!("{name}#f{frame_index}"), "image/png", png);

    if let Ok(mut cache) = frame_cache().lock() {
        cache.insert(key.clone(), cached.clone());
    }
    Ok(cached)
}

/// 协议层入口：只查帧缓存，不解码（未命中由前端重新走命令预热）。
pub(crate) fn cached_frame(path: &Path, page_index: i64, frame_index: u32) -> Option<Cached> {
    let key = reader::cache_key("frame", path, page_index, &frame_index.to_string()).ok()?;
    frame_cache().lock().ok()?.get(&key)
}

fn extension_of(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn png_bytes(image: DynamicImage) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
        .map_err(|error| format!("帧编码失败: {error}"))?;
    Ok(encoded)
}

fn gif_meta(bytes: &[u8]) -> Result<AnimMeta, String> {
    let decoder =
        GifDecoder::new(Cursor::new(bytes)).map_err(|error| format!("GIF 解码失败: {error}"))?;
    let (width, height) = decoder.dimensions();
    let mut frames = decoder.into_frames();

    let mut count = 0u32;
    let mut total = 0u64;
    while let Some(frame) = frames.next() {
        let frame = frame.map_err(|error| format!("GIF 帧读取失败: {error}"))?;
        // Delay 的单位是「毫秒分子/毫秒分母」；gif 解码器固定分母 1，
        // 分母异常（0）时按 100ms 兜底，避免拖条统计被坏数据拖爆。
        let (numer, denom) = frame.delay().numer_denom_ms();
        total += if denom == 0 { 100 } else { u64::from(numer) / u64::from(denom) };
        count += 1;
    }

    Ok(AnimMeta {
        animated: count > 1,
        frames: count,
        total_duration_ms: total,
        width,
        height,
    })
}

fn webp_meta(bytes: &[u8]) -> Result<AnimMeta, String> {
    let decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| format!("webp 解码失败: {error}"))?;
    let (width, height) = decoder.dimensions();
    if !decoder.is_animated() {
        return Ok(AnimMeta::still(width, height));
    }

    let frames = decoder.num_frames();
    Ok(AnimMeta {
        animated: frames > 1,
        frames,
        total_duration_ms: decoder.loop_duration(),
        width,
        height,
    })
}

fn gif_frame_png(bytes: &[u8], frame_index: u32) -> Result<Vec<u8>, String> {
    let decoder =
        GifDecoder::new(Cursor::new(bytes)).map_err(|error| format!("GIF 解码失败: {error}"))?;
    let frames = decoder.into_frames();

    let mut found: Option<Frame> = None;
    for (index, frame) in frames.enumerate() {
        let frame = frame.map_err(|error| format!("GIF 帧读取失败: {error}"))?;
        if index as u32 == frame_index {
            found = Some(frame);
            break;
        }
    }
    let frame = found.ok_or_else(|| "帧序号超出范围".to_string())?;
    png_bytes(DynamicImage::ImageRgba8(frame.into_buffer()))
}

fn webp_frame_png(bytes: &[u8], frame_index: u32) -> Result<Vec<u8>, String> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| format!("webp 解码失败: {error}"))?;
    let (width, height) = decoder.dimensions();
    let buffer_len = decoder
        .output_buffer_size()
        .unwrap_or((width as usize).saturating_mul(height as usize) * 4);
    let mut buffer = vec![0u8; buffer_len];

    // read_frame 顺序前进并在返回值里给当前帧时长；fresh decoder 从第 0 帧开始。
    let mut found: Option<Vec<u8>> = None;
    for index in 0..=frame_index {
        match decoder.read_frame(&mut buffer) {
            Ok(_duration_ms) => {
                if index == frame_index {
                    found = Some(buffer.clone());
                }
            }
            Err(_) => return Err("帧序号超出范围".to_string()),
        }
    }
    let Some(rgba) = found else {
        return Err("帧序号超出范围".to_string());
    };
    png_bytes(DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| "帧缓冲尺寸不匹配".to_string())?,
    ))
}
