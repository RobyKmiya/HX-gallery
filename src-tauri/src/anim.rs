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
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, DynamicImage, Frame, ImageDecoder};

use crate::pagecache::{ByteCache, Cached};
use crate::reader;

/// 帧字节缓存预算。预览帧已降采样（最长边 1280），单帧 PNG 约 0.3–1MB。
const FRAME_CACHE_BUDGET: usize = 160 * 1024 * 1024;
/// meta 缓存条数上限（条目很小，超了整体清空）。
const META_CACHE_LIMIT: usize = 256;
/// 拖条预览帧的最长边。拖条只是选帧预览，松手回原生动画看全分辨率。
const FRAME_PREVIEW_MAX_EDGE: u32 = 1280;
/// 顺序解码会话的常驻数量上限（按 (路径, 页) 寻址，最旧淘汰）。
const SESSION_LIMIT: usize = 4;
/// 每次会话命令的解码预算：到时即返回进度，前端循环续跑，避免单次占用过久。
const SESSION_BUDGET: Duration = Duration::from_secs(3);

/// 顺序帧解码会话：复用同一个解码器增量推进，沿途缓存。
/// 单帧请求各自从头解码是 O(N) 的（动画 webp 只能顺序读），拖到几百帧的
/// 动画上会把 CPU 打满——这就是「帧条卡死」的根源。
/// image 的 `Frames` 内部是 `Box<dyn Iterator>`（没标 Send），但它的 gif 实现
/// `GifFrameIterator` 字段全是 Send 类型（gif::Decoder + ImageBuffer + 标量），
/// 且会话只在互斥锁下由单线程访问——这里显式声明 Send 是安全的。
struct SendFrames(image::Frames<'static>);
unsafe impl Send for SendFrames {}

enum SessionDecoder {
    Gif(SendFrames),
    WebP {
        decoder: image_webp::WebPDecoder<Cursor<Vec<u8>>>,
        buffer: Vec<u8>,
    },
}

struct FrameSession {
    stamp: (i64, u64),
    /// 已解码帧数（下一帧 = cursor）。
    cursor: u32,
    /// 动画已到结尾（再解也没有了）。
    exhausted: bool,
    decoder: SessionDecoder,
}

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

/// 活跃解码会话（最多 SESSION_LIMIT 个，最旧淘汰）。
fn sessions() -> &'static Mutex<Vec<(PathBuf, i64, FrameSession)>> {
    static SESSIONS: OnceLock<Mutex<Vec<(PathBuf, i64, FrameSession)>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct FrameProgress {
    /// 已解码到第 ready 帧（不含），即缓存里有 [0, ready) 的预览帧。
    pub ready: u32,
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
/// 单帧请求是 O(N) 顺序解码（动画 webp 无随机访问），前端拖条请改用
/// `prepare_anim_frames` 的会话路径，这里保留给测试与零星补帧。
pub(crate) fn prepare_frame(path: &Path, page_index: i64, frame_index: u32) -> Result<Cached, String> {
    let key = reader::cache_key("frame", path, page_index, &frame_index.to_string())?;
    if let Ok(mut cache) = frame_cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        return Ok(hit);
    }

    let (name, bytes) = reader::read_page_bytes(path, page_index)?;
    let jpeg = match extension_of(&name).as_str() {
        "gif" => gif_frame_png(&bytes, frame_index)?,
        "webp" => webp_frame_png(&bytes, frame_index)?,
        _ => return Err("该页不是动图格式".to_string()),
    };
    let cached = Cached::new(format!("{name}#f{frame_index}"), "image/jpeg", jpeg);

    if let Ok(mut cache) = frame_cache().lock() {
        cache.insert(key.clone(), cached.clone());
    }
    Ok(cached)
}

/// 命令层入口：用顺序解码会话把帧推进到 `upto`（含），沿途全部进缓存。
///
/// 单帧解码无法随机访问（gif/webp 都只能顺序读），所以「拖到第 300 帧」
/// 不可能只解一帧——这里复用会话里已有的解码器从上次进度继续，到预算
/// （3 秒）或目标即返回；前端拿到 `ready` 后循环续跑，直到覆盖目标。
/// 一次会话中每帧只解一次，整体 O(target)，而不是拖条每格 O(N)。
pub(crate) fn prepare_anim_frames(
    path: &Path,
    page_index: i64,
    upto: u32,
) -> Result<FrameProgress, String> {
    let stamp = reader::file_stamp(path)?;
    let deadline = Instant::now() + SESSION_BUDGET;

    let mut sessions = sessions().lock().map_err(|_| "解码会话锁异常".to_string())?;
    let session = match sessions
        .iter()
        .position(|(stored, index, state)| {
            stored == path && *index == page_index && state.stamp == stamp
        }) {
        Some(position) => sessions.remove(position).2,
        None => create_session(path, page_index, stamp)?,
    };
    let mut session = session;

    while Instant::now() < deadline
        && !session.exhausted
        && session.cursor <= upto
    {
        match session_decode_next(&mut session) {
            Ok(image) => {
                let preview = downscale_preview(image);
                let jpeg = jpeg_bytes(&preview)?;
                let key = reader::cache_key(
                    "frame",
                    path,
                    page_index,
                    &session.cursor.to_string(),
                )?;
                if let Ok(mut cache) = frame_cache().lock() {
                    cache.insert(key, Cached::new("frame".to_string(), "image/jpeg", jpeg));
                }
                session.cursor += 1;
            }
            Err(_) => {
                // 到动画结尾了：之后不再空解。
                session.exhausted = true;
                break;
            }
        }
    }

    let ready = session.cursor;
    sessions.push((path.to_path_buf(), page_index, session));
    while sessions.len() > SESSION_LIMIT {
        sessions.remove(0);
    }
    Ok(FrameProgress { ready })
}

fn create_session(path: &Path, page_index: i64, stamp: (i64, u64)) -> Result<FrameSession, String> {
    let (name, bytes) = reader::read_page_bytes(path, page_index)?;
    match extension_of(&name).as_str() {
        "gif" => {
            let decoder =
                GifDecoder::new(Cursor::new(bytes)).map_err(|error| format!("GIF 解码失败: {error}"))?;
            let frames = decoder.into_frames();
            Ok(FrameSession {
                stamp,
                cursor: 0,
                exhausted: false,
                decoder: SessionDecoder::Gif(SendFrames(frames)),
            })
        }
        "webp" => {
            let decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|error| format!("webp 解码失败: {error}"))?;
            if !decoder.is_animated() {
                return Err("该页不是动画 webp".to_string());
            }
            let buffer_len = decoder.output_buffer_size().unwrap_or_else(|| {
                let (width, height) = decoder.dimensions();
                (width as usize).saturating_mul(height as usize) * 4
            });
            Ok(FrameSession {
                stamp,
                cursor: 0,
                exhausted: false,
                decoder: SessionDecoder::WebP {
                    decoder,
                    buffer: vec![0u8; buffer_len],
                },
            })
        }
        _ => Err("该页不是动图格式".to_string()),
    }
}

fn session_decode_next(session: &mut FrameSession) -> Result<DynamicImage, String> {
    match &mut session.decoder {
        SessionDecoder::Gif(frames) => {
            let frame = (frames.0)
                .next()
                .ok_or_else(|| "帧序号超出范围".to_string())?
                .map_err(|error| format!("GIF 帧读取失败: {error}"))?;
            Ok(DynamicImage::ImageRgba8(frame.into_buffer()))
        }
        SessionDecoder::WebP { decoder, buffer } => {
            decoder
                .read_frame(buffer)
                .map_err(|_| "帧序号超出范围".to_string())?;
            let (width, height) = decoder.dimensions();
            Ok(DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, buffer.clone())
                    .ok_or_else(|| "帧缓冲尺寸不匹配".to_string())?,
            ))
        }
    }
}

/// 预览降采样：拖条只是选帧，松手回原生动画看全分辨率。
fn downscale_preview(image: DynamicImage) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    let longest = width.max(height);
    if longest <= FRAME_PREVIEW_MAX_EDGE {
        return image;
    }
    image.thumbnail(FRAME_PREVIEW_MAX_EDGE, FRAME_PREVIEW_MAX_EDGE)
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

/// 预览帧编码：JPEG q80。体积是 PNG-Fast 的约十分之一（160MB 缓存能装下
/// 数百帧而不是几十帧），编码也更快；拖条预览对有损不敏感，松手回原生动画。
fn jpeg_bytes(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 80);
    encoder
        .encode_image(&image.to_rgb8())
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
    jpeg_bytes(&DynamicImage::ImageRgba8(frame.into_buffer()))
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
    jpeg_bytes(&DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| "帧缓冲尺寸不匹配".to_string())?,
    ))
}
