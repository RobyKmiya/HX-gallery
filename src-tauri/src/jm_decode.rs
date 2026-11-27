//! 18comic（禁漫）图片混淆还原。
//!
//! 站点把原图沿竖直方向切成 num 段再重排，浏览器端用 JS/CSS 拼回去；
//! 我们 curl 拿到的是**混淆后的原图**，直接存进 CBZ 就成一条条错位的画面。
//!
//! 重排规则取自 JMComic-Crawler-Python（`jm_toolkit.py` 的 `get_num` /
//! `decode_and_save`）：
//!
//!   move = h / num（整除）; over = h % num
//!   y_src = h - move*(i+1) - over
//!   y_dst = move*i
//!   i == 0 时 move += over，否则 y_dst += over
//!
//! 即「把竖直分块按倒序摆回来」。
//!
//! **段数不能只信公式？不，恰恰只能信公式。** num 由 `scramble_id` + 作品 id +
//! 文件名算 md5 得出（官方 get_num），输入对输出就对。2026-11-26 曾因
//! scramble_id 正则抓错数/aid 传成专辑 id 导致段数错，当时加了一层「画面自校验」
//! 兜底；2026-11-27 管理员第三次反馈「还是错位」，实测根因就是这层自校验：
//! 真实漫画在缩略图上的连续度打分完全失灵，把公式算对的段数否决掉了。
//! 已删掉自校验，回归官方行为：无条件信任公式（入参也已修正为章节 photo_id）。

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
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let last_byte = digest.last().copied().unwrap_or(0) as usize;
    let ord = HEX[last_byte & 0x0f] as usize;
    (ord % x) * 2 + 2
}

/// 分段方案：(源起始行, 行数, 目标起始行)。
fn plan(height: u32, num: usize) -> Vec<(u32, u32, u32)> {
    if num == 0 {
        return Vec::new();
    }
    let num = num as u32;
    if height < num || num == 0 {
        return Vec::new();
    }
    let base = height / num;
    let over = height % num;
    let mut out = Vec::with_capacity(num as usize);
    for i in 0..num {
        let mut mv = base;
        // y_src 用 base 算（不带头块的补齐量），和 Python 完全一致。
        let y_src = match height.checked_sub(base * (i + 1)).and_then(|v| v.checked_sub(over)) {
            Some(value) => value,
            None => continue,
        };
        let mut y_dst = base * i;
        if i == 0 {
            mv += over;
        } else {
            y_dst += over;
        }
        if mv == 0 || y_src + mv > height || y_dst + mv > height {
            continue;
        }
        out.push((y_src, mv, y_dst));
    }
    out
}

/// 按 num 段重排出一张新图（num == 0 时原样返回）。
fn permute(img: &image::RgbImage, num: usize) -> image::RgbImage {
    let (w, h) = img.dimensions();
    if num == 0 || h < num as u32 {
        return img.clone();
    }
    let mut out = image::RgbImage::new(w, h);
    for (src, len, dst) in plan(h, num) {
        for y in 0..len {
            for x in 0..w {
                out.put_pixel(x, dst + y, *img.get_pixel(x, src + y));
            }
        }
    }
    out
}

/// 选段数 + 还原，原地覆盖 `path`。返回实际使用的段数（0 表示没动）。
///
/// `computed` 是公式（segmentation_num）给出的段数，**无条件信任**。
///
/// 2026-11-27 教训（管理员第三次反馈「还是错位」）：上一版加的「画面自校验」
/// 把正确的段数否决了——64px 缩略图上的连续度打分在真实漫画上完全失灵
/// （实测一张真实本子：还原后 32.3 vs 不动 26.8，还原反而「更不连续」），
/// 于是 trust_computed 过不了、搜索又选中 0 = 不处理，一张都没还原、
/// 整本原样错位入库。合成渐变测试图骗过了打分，真实画面骗不过它。
/// 官方 JMComic-Crawler-Python 从不校验：段数只由 scramble_id + photo_id +
/// 文件名决定，输入对输出就对。入参现已修正为章节 photo_id（jm_api.rs），
/// 照官方信公式。
pub fn decode_image_file(path: &std::path::Path, computed: usize) -> Result<usize, String> {
    if computed == 0 {
        return Ok(0);
    }
    // 走统一的 decode_image：Adobe CMYK JPEG 的反相修正也在里面（1.1.100）。
    let bytes =
        std::fs::read(path).map_err(|e| format!("读图失败（{}）：{e}", path.display()))?;
    let img = crate::archive::decode_image(&bytes, crate::archive::PAGE_DECODE_ALLOC)
        .map_err(|e| format!("读图失败（{}）：{e}", path.display()))?;
    let rgb = img.to_rgb8();
    let out = permute(&rgb, computed);
    out.save(path)
        .map_err(|e| format!("写回还原图失败（{}）：{e}", path.display()))?;
    Ok(computed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

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

    /// 造一张「每行颜色都不同」的图：段序错了一定会被连续度打分看出来。
    fn gradient(height: u32) -> RgbImage {
        let mut img = RgbImage::new(32, height);
        for y in 0..height {
            let shade = (y * 255 / height.max(1)) as u8;
            for x in 0..32 {
                img.put_pixel(x, y, Rgb([shade, shade.wrapping_add(40), shade.wrapping_add(90)]));
            }
        }
        img
    }

    /// 整除时是「分块倒序」，倒序两次回到原样。
    /// 有余额时不是对合变换（首块多吃了 over 行），所以只测整除高度。
    #[test]
    fn permute_is_reverted_by_same_num() {
        for num in [2usize, 4, 6, 8, 10, 12, 14, 16] {
            let original = gradient(num as u32 * 40);
            let scrambled = permute(&original, num);
            let restored = permute(&scrambled, num);
            let (w, h) = original.dimensions();
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(
                        original.get_pixel(x, y),
                        restored.get_pixel(x, y),
                        "num={num} 第 {y} 行还原不一致"
                    );
                }
            }
        }
    }

    /// 带余额时最容易出错的是「有的行没被写、有的行被写两次」——必须每行恰好覆盖一次。
    #[test]
    fn plan_covers_every_row_exactly_once() {
        for num in [2usize, 4, 6, 8, 10, 12, 14, 16] {
            for height in [601u32, 999, 1200, 1080, num as u32 * 7 + 3] {
                if (height as usize) < num {
                    continue;
                }
                let mut cover = vec![0u8; height as usize];
                for (src, len, dst) in plan(height, num) {
                    assert!(src + len <= height, "num={num} h={height} 源越界");
                    assert!(dst + len <= height, "num={num} h={height} 目标越界");
                    for step in 0..len {
                        cover[(dst + step) as usize] += 1;
                    }
                }
                assert!(
                    cover.iter().all(|count| *count == 1),
                    "num={num} h={height} 覆盖不正确：{:?}",
                    cover.iter().filter(|c| **c != 1).count()
                );
            }
        }
    }

    /// 公式段数正确时（整除高度），decode 应一次性还原出原图。
    #[test]
    fn decode_trusts_computed_segmentation() {
        let dir = std::env::temp_dir().join("hx-jm-test");
        let _ = std::fs::create_dir_all(&dir);

        let original = gradient(1200);
        let scrambled = permute(&original, 12);
        let path = dir.join("trust-num.png");
        scrambled.save(&path).expect("写测试图失败");

        let used = decode_image_file(&path, 12).expect("还原失败");
        assert_eq!(used, 12);

        let restored = image::open(&path).expect("读回失败").to_rgb8();
        let (w, h) = original.dimensions();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(
                    original.get_pixel(x, y),
                    restored.get_pixel(x, y),
                    "第 {y} 行还原不一致"
                );
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    /// computed == 0 表示公式判定未混淆：文件必须原样保留。
    #[test]
    fn decode_zero_leaves_file_untouched() {
        let dir = std::env::temp_dir().join("hx-jm-test");
        let _ = std::fs::create_dir_all(&dir);
        let original = gradient(900);
        let path = dir.join("clean.png");
        original.save(&path).expect("写测试图失败");

        let used = decode_image_file(&path, 0).expect("处理失败");
        assert_eq!(used, 0, "未混淆的图不该动");
        let restored = image::open(&path).expect("读回失败").to_rgb8();
        let (w, h) = original.dimensions();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(original.get_pixel(x, y), restored.get_pixel(x, y));
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 18comic 成品全是 webp（2026-11-27 实测整本 39 张 webp）：
    /// image crate 必须能读也能写回 webp，否则还原会整批静默失败。
    #[test]
    fn decode_roundtrip_webp() {
        let dir = std::env::temp_dir().join("hx-jm-test");
        let _ = std::fs::create_dir_all(&dir);

        let original = gradient(480);
        let scrambled = permute(&original, 8);
        let path = dir.join("probe.webp");
        scrambled.save(&path).expect("webp 写盘失败（编码器缺失？）");

        let used = decode_image_file(&path, 8).expect("webp 还原失败");
        assert_eq!(used, 8);
        let restored = image::open(&path).expect("读回失败").to_rgb8();
        let (w, h) = original.dimensions();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(original.get_pixel(x, y), restored.get_pixel(x, y));
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}
