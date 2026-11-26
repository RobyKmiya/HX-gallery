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
//! **段数不能只信公式**：num 由 `scramble_id` + 作品 id + 文件名算 md5 得出，
//! 而 `scramble_id` 随站点改版会变（页面取不到时我们只能退回默认值），文件名
//! 也可能带后缀/查询串。2026-11-26 管理员第二次反馈「还是错位」，就是段数算错：
//! 段数错 → 块数与真实不符 → 怎么倒序都是花的。
//!
//! 所以这里加了一层**画面自校验**：把图缩到很小，用「相邻行差异」衡量连续度，
//! 把候选段数（含 0 = 不处理）逐个试一遍，取最连续的那个。公式给出的段数优先，
//! 只有在它明显没把画面变得连续时才改用搜索出来的结果。

use md5::{Digest, Md5};

/// 站点算法常量（见 JMComic-Crawler-Python JmMagicConstants）。
const SCRAMBLE_268850: u64 = 268850;
const SCRAMBLE_421926: u64 = 421926;

/// 候选段数上限：公式里 x 取 8 或 10，(ord % x) * 2 + 2 最大 20。
const MAX_SEGMENTS: usize = 20;

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

/// 画面纵向连续度：相邻行平均差异，越小越连续。
fn seam_score(img: &image::RgbImage) -> f64 {
    let (w, h) = img.dimensions();
    if h < 2 || w == 0 {
        return 0.0;
    }
    let mut total = 0f64;
    for y in 1..h {
        let mut row = 0f64;
        for x in 0..w {
            let a = img.get_pixel(x, y - 1);
            let b = img.get_pixel(x, y);
            row += (a[0] as i32 - b[0] as i32).abs() as f64
                + (a[1] as i32 - b[1] as i32).abs() as f64
                + (a[2] as i32 - b[2] as i32).abs() as f64;
        }
        total += row / (w as f64 * 3.0);
    }
    total / (h - 1) as f64
}

/// 缩略一份用于打分（宽 64，保持比例）。打分开销必须小：一张图要试十来个段数。
fn thumbnail(img: &image::RgbImage) -> image::RgbImage {
    let (w, h) = img.dimensions();
    if w <= 64 {
        return img.clone();
    }
    let new_h = ((h as u64 * 64) / w as u64).max(2) as u32;
    image::imageops::resize(img, 64, new_h, image::imageops::FilterType::Triangle)
}

/// 选段数 + 还原，原地覆盖 `path`。返回实际使用的段数（0 表示没动）。
///
/// `computed` 是公式算出来的段数，作为优先候选。
pub fn decode_image_file(path: &std::path::Path, computed: usize) -> Result<usize, String> {
    let img = image::open(path).map_err(|e| format!("读图失败（{}）：{e}", path.display()))?;
    let rgb = img.to_rgb8();
    let (_, height) = rgb.dimensions();

    let small = thumbnail(&rgb);
    let identity = seam_score(&small);
    let mut best = computed;
    let mut best_score = if computed == 0 {
        identity
    } else {
        seam_score(&permute(&small, computed))
    };
    // 公式算出来的段数明显让画面更连续 → 直接信它。
    let trust_computed = computed > 0 && best_score <= identity * 0.82;

    if !trust_computed {
        // 公式不可信（页面没给 scramble_id / 文件名对不上）：把候选段数都试一遍。
        if identity < best_score {
            best = 0;
            best_score = identity;
        }
        let mut candidate = 2usize;
        while candidate <= MAX_SEGMENTS {
            if candidate != computed && candidate <= height as usize {
                let score = seam_score(&permute(&small, candidate));
                // 平手时保留更小的段数：真实段数不会凭空变大。
                if score < best_score * 0.985 {
                    best = candidate;
                    best_score = score;
                }
            }
            candidate += 2;
        }
    }

    if best == 0 {
        return Ok(0);
    }
    let out = permute(&rgb, best);
    out.save(path)
        .map_err(|e| format!("写回还原图失败（{}）：{e}", path.display()))?;
    Ok(best)
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

    #[test]
    fn auto_detect_repairs_wrong_segmentation() {
        let dir = std::env::temp_dir().join("hx-jm-test");
        let _ = std::fs::create_dir_all(&dir);

        // 真实段数 12，但公式给了一个错的 8（模拟 scramble_id/文件名对不上）。
        let original = gradient(1200);
        let scrambled = permute(&original, 12);
        let path = dir.join("wrong-num.png");
        scrambled.save(&path).expect("写测试图失败");

        let used = decode_image_file(&path, 8).expect("还原失败");
        assert_eq!(used, 12, "应当自动纠正到真实段数");

        let restored = image::open(&path).expect("读回失败").to_rgb8();
        let (w, h) = original.dimensions();
        let mut same = 0usize;
        for y in 0..h {
            for x in 0..w {
                if original.get_pixel(x, y) == restored.get_pixel(x, y) {
                    same += 1;
                }
            }
        }
        assert!(
            same as f64 >= (w * h) as f64 * 0.98,
            "还原结果与原始图不一致：{same}/{}",
            w * h
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn auto_detect_keeps_clean_image_untouched() {
        let dir = std::env::temp_dir().join("hx-jm-test");
        let _ = std::fs::create_dir_all(&dir);
        let original = gradient(900);
        let path = dir.join("clean.png");
        original.save(&path).expect("写测试图失败");

        // 公式说 10 段，但这张图本来就没混淆 → 不应该被倒序。
        let used = decode_image_file(&path, 10).expect("处理失败");
        assert_eq!(used, 0, "本来没混淆的图不该动");
        let _ = std::fs::remove_file(&path);
    }
}
