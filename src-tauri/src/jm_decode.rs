//! 18comic（禁漫）图片混淆还原。
//!
//! 管理员 2026-11-26 反馈：下载下来的漫画被切成一条条横向条带、顺序全乱
//! （见截图）。根因是站点对原图做了竖直切片 + 按算法打乱，浏览器端用 JS/CSS
//! 拼回正常画面；我们 curl 拿到的是**混淆后的原图**，直接存进 CBZ 就错位。
//!
//! 算法取自开源项目 JMComic-Crawler-Python（jm_toolkit.py 的 get_num /
//! decode_and_save），是这套站点混淆的公开逆向结果：
//!
//! - 分割数 num：
//!   - aid < scramble_id        → 0（未混淆，直接存）
//!   - aid < 268850             → 10
//!   - 268850 <= aid < 421926   → x=10
//!   - aid >= 421926            → x=8
//!     num = (ord(md5(f"{aid}{filename}")[-1]) % x) * 2 + 2
//!   `filename` 是不带扩展名的文件名（如 `00042`）。
//! - 还原：把高为 h 的图切成 num 段竖条，第 i 段的源位置和目标位置按下式
//!   重排（最后一段带余数 over）：
//!     move = floor(h/num); over = h % num
//!     y_src = h - move*(i+1) - over
//!     y_dst = move*i
//!     i==0 时 move += over，否则 y_dst += over
//!     把 (0,y_src,w,y_src+move) 贴到 (0,y_dst,w,y_dst+move)

use md5::{Digest, Md5};

/// 站点算法常量（见 JMComic-Crawler-Python JmMagicConstants）。
const SCRAMBLE_268850: u64 = 268850;
const SCRAMBLE_421926: u64 = 421926;

/// 计算一张图的竖直切片数。返回 0 表示未混淆。
pub fn segmentation_num(scramble_id: u64, aid: u64, filename: &str) -> usize {
    if aid < scramble_id {
        return 0;
    }
    if aid < SCRAMBLE_268850 {
        return 10;
    }
    let x: usize = if aid < SCRAMBLE_421926 { 10 } else { 8 };
    let mut hasher = Md5::new();
    hasher.update(format!("{aid}{filename}").as_bytes());
    let digest = hasher.finalize();
    // Python 是 ord(md5(...).hexdigest()[-1])：hex 串最后一个字符的 ASCII。
    // hexdigest 末字符 == 最后一个字节的低 4 位对应的 hex 字符。
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let last_byte = digest.last().copied().unwrap_or(0) as usize;
    let ord = HEX[last_byte & 0x0f] as usize;
    (ord % x) * 2 + 2
}

/// 把混淆图解码成正常图，原地覆盖 `path`。返回是否真的做了还原。
pub fn decode_image_file(path: &std::path::Path, num: usize) -> Result<bool, String> {
    if num == 0 {
        return Ok(false);
    }
    let img = image::open(path).map_err(|e| format!("读图失败（{}）：{e}", path.display()))?;
    let (w, h) = (img.width(), img.height());
    if h < num as u32 {
        return Ok(false);
    }
    let mut out = image::RgbImage::new(w, h);
    let rgb = img.to_rgb8();
    let move_base = h / num as u32;
    let over = h % num as u32;
    for i in 0..num as u32 {
        let mut mv = move_base;
        let y_src = h - mv * (i + 1) - over;
        let mut y_dst = mv * i;
        if i == 0 {
            mv += over;
        } else {
            y_dst += over;
        }
        for y in 0..mv {
            for x in 0..w {
                let src_y = y_src + y;
                let dst_y = y_dst + y;
                if src_y < h && dst_y < h {
                    out.put_pixel(x, dst_y, *rgb.get_pixel(x, src_y));
                }
            }
        }
    }
    out.save(path)
        .map_err(|e| format!("写回还原图失败（{}）：{e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmentation_num_matches_jmcomic_reference() {
        // 参考值由 JMComic-Crawler-Python 的 get_num 在同参数下算出。
        assert_eq!(segmentation_num(220980, 1492995, "00001"), 12);
        assert_eq!(segmentation_num(220980, 1492995, "00042"), 16);
        assert_eq!(segmentation_num(220980, 1492995, "1"), 8);
        assert_eq!(segmentation_num(220980, 549672, "00001"), 2);
        assert_eq!(segmentation_num(220980, 549672, "00042"), 10);
        assert_eq!(segmentation_num(220980, 422866, "00001"), 10);
        assert_eq!(segmentation_num(220980, 699959, "00042"), 6);
        // aid 小于 scramble_id 视为未混淆。
        assert_eq!(segmentation_num(220980, 180459, "00042"), 0);
        assert_eq!(segmentation_num(1492996, 1492995, "00042"), 0);
        // 老本子固定 10 段（268850 之前）。
        assert_eq!(segmentation_num(220980, 250000, "x"), 10);
    }
}
