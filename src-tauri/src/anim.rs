//! 动图（gif / 动画 webp）的轻量元数据：帧数 / 总时长 / 原始尺寸。
//!
//! 播放本身**不在这里做**：动图页直接把压缩包里的原始字节交给 webview，
//! 由 Chromium 内核原生播放（读原图 gif / 动画 webp，帧时序、合成、透明
//! 全部由内核处理）。这和 HoneyView / Bandizip 的做法一致——不重编码、
//! 不逐帧搬运，所以既不卡也不闪。
//!
//! 之前那套「后端逐帧解成 PNG/JPEG 再让前端自绘」的路线已经废弃：它要为
//! 每一帧做一次解码 + 重编码 + 一次协议往返，几百帧的 AI 动图会把 CPU 打满，
//! 帧与帧之间换 `<img>` 还会闪黑。
//!
//! 这里只提供播放模式需要的元数据：
//! - `total_duration_ms`：播一遍要多久，播放模式据此决定「播完翻页」的时刻；
//! - `width` / `height`：原始尺寸，供缩放与占位使用。
//!
//! 两者都只用「块头信息」取得，不逐帧解像素，所以进页很快。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::reader;

/// meta 缓存条数上限（条目很小，超了整体清空）。
const META_CACHE_LIMIT: usize = 512;
/// GIF 帧延迟为 0 时的兜底时长（毫秒）。浏览器对 0 延迟同样会兜底到约 100ms，
/// 否则「按 0 毫秒连播」会看起来像卡住或瞬间跳过。
const GIF_ZERO_DELAY_MS: u64 = 100;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnimMeta {
    pub animated: bool,
    pub frames: u32,
    /// 播一遍的总时长（毫秒）。非动图或取不到时为 0。
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

    let (name, bytes) = reader::page_bytes_for_decode(path, page_index)?;
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

fn extension_of(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// GIF 元数据：只扫块头（帧延迟 / 帧数），不逐帧解像素。
///
/// `next_frame_info` 只解析 Graphic Control Extension 与应用扩展，跳过 LZW
/// 数据，因此几百帧的 AI 动图也是毫秒级完成，不会像整段解码那样卡住进页。
fn gif_meta(bytes: &[u8]) -> Result<AnimMeta, String> {
    let mut decoder =
        gif::Decoder::new(Cursor::new(bytes)).map_err(|error| format!("GIF 解码失败: {error}"))?;
    let width = u32::from(decoder.width());
    let height = u32::from(decoder.height());

    let mut frames = 0u32;
    let mut total = 0u64;
    loop {
        match decoder.next_frame_info() {
            Ok(Some(frame)) => {
                // gif 帧延迟单位是 10ms；0 按 100ms 兜底。
                let millis = u64::from(frame.delay) * 10;
                total += if millis == 0 { GIF_ZERO_DELAY_MS } else { millis };
                frames += 1;
            }
            // 正常读到结尾。
            Ok(None) => break,
            // 坏帧之后的块结构不再可信，用已统计到的帧数/时长。
            Err(_) => break,
        }
    }

    Ok(AnimMeta {
        animated: frames > 1,
        frames,
        total_duration_ms: total,
        width,
        height,
    })
}

/// 动画 webp 元数据：`num_frames` / `loop_duration` 都只读 ANMF 头，不整段解码。
fn webp_meta(bytes: &[u8]) -> Result<AnimMeta, String> {
    let decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| format!("webp 解码失败: {error}"))?;
    let (width, height) = decoder.dimensions();
    if !decoder.is_animated() {
        return Ok(AnimMeta::still(width, height));
    }

    let frames = decoder.num_frames();
    let total = decoder.loop_duration();
    // 少数文件不给 loop 时长；退化成「帧数 × 100ms」保证播放模式还能推进。
    let total_duration_ms = if total > 0 {
        total
    } else {
        u64::from(frames) * GIF_ZERO_DELAY_MS
    };
    Ok(AnimMeta {
        animated: frames > 1,
        frames,
        total_duration_ms,
        width,
        height,
    })
}
