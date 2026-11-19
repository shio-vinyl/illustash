//! 在小网格上做确定性的加权聚类，保留图中实际存在的颜色。
use image::RgbImage;
use std::collections::BTreeMap;

const MAX_COLORS: usize = 5;
const GRID: u32 = 24;
// 颜色的量化只用于挑初始中心，不用于聚类或最终色值。
const MIN_DISTANCE: u32 = 28 * 28;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Swatch {
    pub rgb: [u8; 3],
    pub share: f64,
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3).map(|i| (i32::from(a[i]) - i32::from(b[i])).pow(2) as u32).sum()
}

fn chroma(rgb: [u8; 3]) -> u8 {
    rgb.into_iter().max().unwrap_or(0) - rgb.into_iter().min().unwrap_or(0)
}

pub(super) fn extract(image: &RgbImage) -> Vec<Swatch> {
    let (width, height) = image.dimensions();
    let (columns, rows) = (width.min(GRID), height.min(GRID));
    let mut pixels = Vec::with_capacity((columns * rows) as usize);
    let mut bins = BTreeMap::<[u8; 3], (usize, [u8; 3])>::new();
    for y in 0..rows {
        for x in 0..columns {
            let pixel = image.get_pixel((2 * x + 1) * width / (2 * columns), (2 * y + 1) * height / (2 * rows)).0;
            pixels.push(pixel);
            let bin = bins.entry(pixel.map(|v| v >> 5)).or_insert((0, pixel));
            bin.0 += 1;
        }
    }
    let Some(&(.., first)) = bins.values().max_by_key(|(n, _)| n) else { return Vec::new() };
    let mut centers = vec![first];
    while centers.len() < MAX_COLORS {
        let next = bins
            .values()
            .map(|(n, pixel)| {
                let dist = centers.iter().map(|c| distance(*c, *pixel)).min().unwrap_or(0);
                (dist, *n, *pixel)
            })
            .filter(|(dist, _, _)| *dist >= MIN_DISTANCE)
            .max_by_key(|(dist, n, _)| u64::from(*dist).pow(2) * *n as u64);
        let Some((_, _, pixel)) = next else { break };
        centers.push(pixel);
    }
    let mut assigned = vec![0; pixels.len()];
    let mut counts = [0u32; MAX_COLORS];
    for _ in 0..3 {
        counts.fill(0);
        let mut sums = [[0u32; 3]; MAX_COLORS];
        for (pixel, cluster) in pixels.iter().zip(&mut assigned) {
            *cluster = centers
                .iter()
                .enumerate()
                .min_by_key(|(_, center)| distance(*pixel, **center))
                .map_or(0, |(index, _)| index);
            counts[*cluster] += 1;
            for (sum, channel) in sums[*cluster].iter_mut().zip(pixel) {
                *sum += u32::from(*channel);
            }
        }
        for (index, center) in centers.iter_mut().enumerate() {
            if counts[index] != 0 {
                *center = sums[index].map(|sum| (sum / counts[index]) as u8);
            }
        }
    }
    // 取最靠近质心的样本。输出来自图片，即使相邻色合在一组也不会造出灰泥色。
    let mut nearest = [u32::MAX; MAX_COLORS];
    let mut representatives = [[0u8; 3]; MAX_COLORS];
    for (pixel, cluster) in pixels.iter().zip(assigned) {
        let dist = distance(*pixel, centers[cluster]);
        if dist < nearest[cluster] {
            nearest[cluster] = dist;
            representatives[cluster] = *pixel;
        }
    }
    let mut palette: Vec<Swatch> = counts
        .into_iter()
        .zip(representatives)
        .filter(|(count, _)| *count > 0)
        .map(|(count, rgb)| Swatch { rgb, share: f64::from(count) / pixels.len() as f64 })
        .collect();
    // 第一色用于底色。较鲜艳的颜色最多得到约三倍权重，少量高饱和噪声不会压过画面的主要区域。
    let score = |swatch: &Swatch| swatch.share * (1.0 + f64::from(chroma(swatch.rgb)) / 128.0);
    palette.sort_by(|a, b| score(b).total_cmp(&score(a)));
    palette
}

pub(super) fn hex([red, green, blue]: [u8; 3]) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// 红、黄、绿、青、蓝、紫六段围绕各自的色相；最后单列灰度。段内按 HSL 明度升序。
pub(super) fn rank(rgb: [u8; 3]) -> i64 {
    let [r, g, b] = rgb.map(i64::from);
    let high = r.max(g).max(b);
    let low = r.min(g).min(b);
    let span = high - low;
    let bucket = if span <= 12 {
        6
    } else {
        let offset = if high == r {
            g - b
        } else if high == g {
            b - r + 2 * span
        } else {
            r - g + 4 * span
        };
        (2 * offset + span).div_euclid(2 * span).rem_euclid(6)
    };
    bucket * 511 + high + low
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn chooses_a_dominant_color_and_reports_area_shares() {
        let image = RgbImage::from_fn(24, 24, |x, _| {
            Rgb(if x < 11 {
                [242, 242, 240]
            } else if x < 20 {
                [204, 0, 40]
            } else {
                [0, 180, 0]
            })
        });
        let colors = extract(&image);
        assert_eq!(colors[0], Swatch { rgb: [204, 0, 40], share: 9.0 / 24.0 });
        assert_eq!(colors.len(), 3);
        assert!((colors.iter().map(|v| v.share).sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn keeps_grayscale_and_sorts_by_hue_then_lightness() {
        let image = RgbImage::from_fn(24, 24, |x, _| Rgb(if x < 16 { [80; 3] } else { [240; 3] }));
        assert_eq!(extract(&image)[0].rgb, [80; 3]);
        let ranks =
            [[128, 0, 12], [255, 0, 5], [255, 255, 0], [0, 255, 0], [0, 255, 255], [0, 0, 255], [255, 0, 255], [0; 3]]
                .map(rank);
        assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(extract(&RgbImage::new(0, 0)), []);
    }
}
