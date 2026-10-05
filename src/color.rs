//! 色の選択パネル用の HSV ⇔ RGB 変換と既定の色見本。

use crate::config::Rgb;

/// 色相 (0～360)・彩度 (0～1)・明度 (0～1)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

pub fn to_hsv(c: Rgb) -> Hsv {
    let (r, g, b) = (c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsv { h, s: if max == 0.0 { 0.0 } else { d / max }, v: max }
}

pub fn to_rgb(hsv: Hsv) -> Rgb {
    let h = hsv.h.rem_euclid(360.0);
    let (s, v) = (hsv.s.clamp(0.0, 1.0), hsv.v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |t: f32| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(f(r), f(g), f(b))
}

/// よく使う色の見本 (ボタン背景向けの暗い色と、文字向けの明るい色を半々)。
pub const PRESETS: [Rgb; 12] = [
    Rgb(60, 60, 60),
    Rgb(32, 33, 37),
    Rgb(40, 52, 74),
    Rgb(36, 62, 52),
    Rgb(74, 44, 52),
    Rgb(70, 56, 32),
    Rgb(180, 180, 180),
    Rgb(255, 255, 255),
    Rgb(140, 196, 255),
    Rgb(150, 220, 170),
    Rgb(255, 170, 160),
    Rgb(250, 210, 120),
];

/// 彩度 (横)・明度 (縦) の平面を BGRA (乗算済み・不透明) で作る。
pub fn sv_plane(hue: f32, width: usize, height: usize) -> Vec<u8> {
    let mut data = vec![0u8; width * height * 4];
    for y in 0..height {
        let v = 1.0 - y as f32 / (height - 1).max(1) as f32;
        for x in 0..width {
            let s = x as f32 / (width - 1).max(1) as f32;
            let c = to_rgb(Hsv { h: hue, s, v });
            let i = (y * width + x) * 4;
            data[i..i + 4].copy_from_slice(&[c.2, c.1, c.0, 255]);
        }
    }
    data
}

/// 色相の帯を BGRA で作る。
pub fn hue_strip(width: usize, height: usize) -> Vec<u8> {
    let mut data = vec![0u8; width * height * 4];
    for x in 0..width {
        let c = to_rgb(Hsv { h: 360.0 * x as f32 / width as f32, s: 1.0, v: 1.0 });
        for y in 0..height {
            let i = (y * width + x) * 4;
            data[i..i + 4].copy_from_slice(&[c.2, c.1, c.0, 255]);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for c in PRESETS.iter().copied().chain([Rgb(0, 0, 0), Rgb(255, 0, 0), Rgb(0, 255, 0), Rgb(0, 0, 255), Rgb(12, 200, 99)]) {
            assert_eq!(to_rgb(to_hsv(c)), c, "{c:?}");
        }
    }

    #[test]
    fn known_values() {
        assert_eq!(to_hsv(Rgb(255, 0, 0)), Hsv { h: 0.0, s: 1.0, v: 1.0 });
        assert_eq!(to_rgb(Hsv { h: 120.0, s: 1.0, v: 1.0 }), Rgb(0, 255, 0));
        assert_eq!(to_hsv(Rgb(60, 60, 60)).s, 0.0);
    }
}
