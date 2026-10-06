//! 半透明の背景 (壁紙ぼかし) と背景画像のぼかしの元になる画素の生成。
//!
//! - 壁紙ぼかし: ウィンドウのいるモニターの壁紙を、Windows の配置方法 (塗りつぶし・
//!   ページ幅に合わせる・拡大して表示・中央・並べて表示・スパン) どおりに縮小キャンバスへ描き、
//!   ぼかした画像を作る。ウィンドウはその中の自分の位置にあたる部分を描くので、移動しても
//!   作り直さない。
//! - ぼかしは 3 回の箱型ぼかしでガウスぼかしを近似する (強さによらず画素数に比例する計算量)。
//!
//! 画素はすべて BGRA (乗算済み)。

use windows::Win32::Foundation::{GENERIC_READ, RECT};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH,
    DWPOS_TILE, DesktopWallpaper, IDesktopWallpaper,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::platform::icon::Pixels;
use crate::platform::wide::to_wide;

/// ぼかす画像の長辺の上限 (画素)。ぼかした後で拡大して描くので、これで十分に滑らか。
pub const BLUR_LIMIT: u32 = 960;

/// 壁紙をモニターの大きさで描いた縮小キャンバス。
pub struct WallpaperCanvas {
    pub pixels: Pixels,
    /// キャンバスが表すモニターの矩形 (物理座標)
    pub monitor: RECT,
    /// 物理ピクセル → キャンバス画素の倍率
    pub factor: f32,
    /// 作り直しの判定用 (壁紙のパスと配置)
    pub key: String,
}

/// `monitor` (物理座標) に表示されている壁紙を、長辺 `limit` 以内のキャンバスに描く。
/// 壁紙が単色・取得できない場合は背景色で塗ったキャンバスを返す。
pub fn wallpaper_canvas(monitor: RECT, limit: u32) -> Option<WallpaperCanvas> {
    let (mw, mh) = (
        (monitor.right - monitor.left).max(1) as f32,
        (monitor.bottom - monitor.top).max(1) as f32,
    );
    let factor = (limit as f32 / mw.max(mh)).min(1.0);
    let (cw, ch) = (
        (mw * factor).round().max(1.0) as u32,
        (mh * factor).round().max(1.0) as u32,
    );
    let info = wallpaper_info(monitor);
    let key = format!(
        "{}|{:?}|{:?}|{}x{}",
        info.path, info.position.0, info.span, cw, ch
    );

    let mut pixels = solid(cw, ch, info.background);
    if !info.path.is_empty()
        && let Some((iw, ih)) = image_size(&info.path)
    {
        let (iw, ih) = (iw as f32, ih as f32);
        // 配置の基準となる領域 (スパンなら全モニターを覆う矩形)
        let area = info.span.unwrap_or(monitor);
        let (aw, ah) = (
            (area.right - area.left) as f32,
            (area.bottom - area.top) as f32,
        );
        let (ox, oy) = (
            (area.left - monitor.left) as f32,
            (area.top - monitor.top) as f32,
        );
        let place = |w: f32, h: f32| (ox + (aw - w) / 2.0, oy + (ah - h) / 2.0, w, h);
        let p = info.position;
        let (x, y, w, h) = if p == DWPOS_FILL || p == DWPOS_SPAN {
            let s = (aw / iw).max(ah / ih);
            place(iw * s, ih * s)
        } else if p == DWPOS_FIT {
            let s = (aw / iw).min(ah / ih);
            place(iw * s, ih * s)
        } else if p == DWPOS_STRETCH {
            (ox, oy, aw, ah)
        } else if p == DWPOS_CENTER {
            place(iw, ih)
        } else {
            (ox, oy, iw, ih) // 並べて表示: 左上から敷き詰める
        };
        let (dw, dh) = (
            (w * factor).round().max(1.0) as u32,
            (h * factor).round().max(1.0) as u32,
        );
        if let Some(img) = decode_scaled(&info.path, dw, dh) {
            let (dx, dy) = ((x * factor).round() as i32, (y * factor).round() as i32);
            if p == DWPOS_TILE {
                let mut ty = dy;
                while ty < ch as i32 {
                    let mut tx = dx;
                    while tx < cw as i32 {
                        blit(&mut pixels, &img, tx, ty);
                        tx += img.width.max(1);
                    }
                    ty += img.height.max(1);
                }
            } else {
                blit(&mut pixels, &img, dx, dy);
            }
        }
    }
    Some(WallpaperCanvas {
        pixels,
        monitor,
        factor,
        key,
    })
}

struct WallpaperInfo {
    path: String,
    position: DESKTOP_WALLPAPER_POSITION,
    background: [u8; 3],
    /// スパンの場合は全モニターを覆う矩形
    span: Option<RECT>,
}

/// モニターの壁紙のパス・配置・背景色。IDesktopWallpaper が使えなければ従来の API で取る。
fn wallpaper_info(monitor: RECT) -> WallpaperInfo {
    let mut info = WallpaperInfo {
        path: String::new(),
        position: DWPOS_FILL,
        background: [0, 0, 0],
        span: None,
    };
    // SAFETY: COM 呼び出し。返された文字列は CoTaskMemFree で解放する
    unsafe {
        if let Ok(dw) =
            CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_INPROC_SERVER)
        {
            if let Ok(c) = dw.GetBackgroundColor() {
                info.background = [
                    (c.0 & 0xFF) as u8,
                    ((c.0 >> 8) & 0xFF) as u8,
                    ((c.0 >> 16) & 0xFF) as u8,
                ];
            }
            info.position = dw.GetPosition().unwrap_or(DWPOS_FILL);
            let count = dw.GetMonitorDevicePathCount().unwrap_or(0);
            let mut union: Option<RECT> = None;
            for i in 0..count {
                let Ok(id) = dw.GetMonitorDevicePathAt(i) else {
                    continue;
                };
                if let Ok(r) = dw.GetMonitorRECT(PCWSTR(id.0)) {
                    union = Some(match union {
                        None => r,
                        Some(u) => RECT {
                            left: u.left.min(r.left),
                            top: u.top.min(r.top),
                            right: u.right.max(r.right),
                            bottom: u.bottom.max(r.bottom),
                        },
                    });
                    if (r == monitor || (info.path.is_empty() && overlaps(r, monitor)))
                        && let Ok(p) = dw.GetWallpaper(PCWSTR(id.0))
                    {
                        info.path = take_pwstr(p);
                    }
                }
                CoTaskMemFree(Some(id.0 as *const _));
            }
            if info.position == DWPOS_SPAN {
                info.span = union;
            }
        }
        if info.path.is_empty() {
            // スライドショーなどで取れない場合は、現在表示中の壁紙ファイル
            let mut buf = [0u16; 1024];
            if SystemParametersInfoW(
                SPI_GETDESKWALLPAPER,
                buf.len() as u32,
                Some(buf.as_mut_ptr() as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
                info.path = String::from_utf16_lossy(&buf[..n]);
            }
        }
    }
    info
}

fn overlaps(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    // SAFETY: COM が確保した NUL 終端文字列
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn wic() -> Option<IWICImagingFactory> {
    // SAFETY: WIC ファクトリの生成
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() }
}

/// 画像ファイルの最初のフレーム。読めなければ None。
pub fn open_frame(wic: &IWICImagingFactory, path: &str) -> Option<IWICBitmapFrameDecode> {
    let wide = to_wide(path);
    // SAFETY: WIC によるファイルの読込のみ
    unsafe {
        wic.CreateDecoderFromFilename(
            PCWSTR(wide.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
        .ok()?
        .GetFrame(0)
        .ok()
    }
}

/// 画像ファイルの寸法 (ピクセル)。読めなければ None。
pub fn image_size(path: &str) -> Option<(u32, u32)> {
    let frame = open_frame(&wic()?, path)?;
    let (mut w, mut h) = (0, 0);
    // SAFETY: 寸法の取得のみ
    unsafe { frame.GetSize(&mut w, &mut h) }.ok()?;
    Some((w, h))
}

/// 画像を `w`×`h` に縮小 (拡大) して BGRA で読む。
pub fn decode_scaled(path: &str, w: u32, h: u32) -> Option<Pixels> {
    let wic = wic()?;
    let frame = open_frame(&wic, path)?;
    // SAFETY: WIC による拡大縮小・形式変換
    unsafe {
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, w, h, WICBitmapInterpolationModeFant)
            .ok()?;
        let converter = wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )
            .ok()?;
        let mut data = vec![0u8; (w * h * 4) as usize];
        converter
            .CopyPixels(std::ptr::null(), w * 4, &mut data)
            .ok()?;
        Some(Pixels {
            width: w as i32,
            height: h as i32,
            data,
        })
    }
}

/// 画像を長辺 `limit` 以内に縮小して読む (背景画像のぼかし用)。
pub fn decode_limited(path: &str, limit: u32) -> Option<Pixels> {
    let (w, h) = image_size(path)?;
    let s = (limit as f32 / w.max(h) as f32).min(1.0);
    decode_scaled(
        path,
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    )
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Pixels {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        data.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    Pixels {
        width: w as i32,
        height: h as i32,
        data,
    }
}

/// `src` を `dst` の (x, y) へ不透明として重ねる (はみ出す部分は切り捨て)。
fn blit(dst: &mut Pixels, src: &Pixels, x: i32, y: i32) {
    for sy in 0..src.height {
        let dy = y + sy;
        if dy < 0 || dy >= dst.height {
            continue;
        }
        let sx0 = (-x).max(0);
        let sx1 = src.width.min(dst.width - x);
        if sx0 >= sx1 {
            continue;
        }
        let s = ((sy * src.width + sx0) * 4) as usize;
        let d = ((dy * dst.width + x + sx0) * 4) as usize;
        let n = ((sx1 - sx0) * 4) as usize;
        dst.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
    }
}

/// ぼかしの強さ (0～100) に応じて縮小してからぼかす。戻り値の画像は元の 1/k の大きさ。
/// 強いぼかしほど細部は消えるので、縮小して計算量を抑えても見た目は変わらない。
/// 標準偏差は画像の長辺に比例させる (100 で長辺の 2.5%)。
pub fn blurred(p: &Pixels, strength: i32) -> Pixels {
    let strength = strength.clamp(0, 100);
    if strength == 0 {
        return Pixels {
            width: p.width,
            height: p.height,
            data: p.data.clone(),
        };
    }
    let k = 1 + strength / 35;
    let small = downscale(p, k);
    let sigma = strength as f32 / 100.0 * 0.025 * p.width.max(p.height) as f32 / k as f32;
    blur(&small, sigma)
}

/// k×k 画素の平均で 1/k に縮小する。
pub fn downscale(p: &Pixels, k: i32) -> Pixels {
    if k <= 1 {
        return Pixels {
            width: p.width,
            height: p.height,
            data: p.data.clone(),
        };
    }
    let (w, h) = ((p.width / k).max(1), (p.height / k).max(1));
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            for dy in 0..k {
                let row = ((y * k + dy).min(p.height - 1) * p.width) as usize;
                for dx in 0..k {
                    let i = (row + (x * k + dx).min(p.width - 1) as usize) * 4;
                    for (s, &v) in sum.iter_mut().zip(&p.data[i..i + 4]) {
                        *s += v as u32;
                    }
                }
            }
            let n = (k * k) as u32;
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                data[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    Pixels {
        width: w,
        height: h,
        data,
    }
}

/// ガウスぼかし (標準偏差 `sigma` 画素) を 3 回の箱型ぼかしで近似する。端は端の画素を延長する。
pub fn blur(p: &Pixels, sigma: f32) -> Pixels {
    let mut out = Pixels {
        width: p.width,
        height: p.height,
        data: p.data.clone(),
    };
    if sigma < 0.5 || p.width <= 1 || p.height <= 1 {
        return out;
    }
    // 縦方向は転置して横方向として処理する (メモリを連続して読むので速い)
    let (w, h) = (p.width as usize, p.height as usize);
    let mut a = vec![0u8; out.data.len()];
    let mut t = vec![0u8; out.data.len()];
    for radius in box_radii(sigma) {
        box_pass(&out.data, &mut a, w, h, radius);
        transpose(&a, &mut t, w, h);
        box_pass(&t, &mut a, h, w, radius);
        transpose(&a, &mut out.data, h, w);
    }
    out
}

/// 標準偏差 `sigma` のガウスぼかしに相当する 3 回分の箱の半径。
fn box_radii(sigma: f32) -> [usize; 3] {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f32 - 12.0 * lower as f32 - 9.0)
        / (-4.0 * lower as f32 - 4.0))
        .round() as i32;
    let mut r = [0usize; 3];
    for (i, v) in r.iter_mut().enumerate() {
        let size = if (i as i32) < m { lower } else { upper };
        *v = ((size.max(1) - 1) / 2) as usize;
    }
    r
}

/// 横方向の箱型ぼかし (移動平均)。端の外側は端の画素を延長する。
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    // 除算は遅いので、逆数を掛けてシフトする (24 ビット固定小数点)
    let inv = (1u64 << 24) / (2 * r + 1) as u64;
    for y in 0..h {
        let row = &src[y * w * 4..(y + 1) * w * 4];
        let out = &mut dst[y * w * 4..(y + 1) * w * 4];
        let px = |i: usize| &row[i.min(w - 1) * 4..i.min(w - 1) * 4 + 4];
        let mut sum = [0u32; 4];
        for k in 0..=2 * r {
            let p = px(k.saturating_sub(r));
            for c in 0..4 {
                sum[c] += p[c] as u32;
            }
        }
        for x in 0..w {
            for c in 0..4 {
                out[x * 4 + c] = ((sum[c] as u64 * inv + (1 << 23)) >> 24).min(255) as u8;
            }
            let (old, new) = (px(x.saturating_sub(r)), px(x + r + 1));
            for c in 0..4 {
                sum[c] = sum[c] + new[c] as u32 - old[c] as u32;
            }
        }
    }
}

/// w×h の画素を h×w へ転置する。
fn transpose(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let s = (y * w + x) * 4;
            let d = (x * h + y) * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: i32, h: i32) -> Pixels {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 4 + y / 4) % 2 == 0 { 255 } else { 0 };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Pixels {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn blur_keeps_flat_color_and_alpha() {
        let flat = solid(20, 10, [10, 200, 30]);
        let b = blur(&flat, 5.0);
        assert_eq!(b.data, flat.data, "単色はぼかしても変わらない");
    }

    #[test]
    fn blur_smooths_and_preserves_mean() {
        let c = checker(64, 64);
        let b = blur(&c, 6.0);
        let mean = |p: &Pixels| {
            p.data.chunks(4).map(|px| px[0] as u64).sum::<u64>() / (p.width * p.height) as u64
        };
        assert!(
            (mean(&b) as i64 - mean(&c) as i64).abs() <= 3,
            "明るさの平均はほぼ保たれる"
        );
        let (min, max) = b
            .data
            .chunks(4)
            .skip(64 * 20)
            .take(64 * 20)
            .fold((255, 0), |(lo, hi), px| (lo.min(px[0]), hi.max(px[0])));
        assert!(max - min < 60, "市松模様がならされる: {min}..{max}");
        assert!(b.data.chunks(4).all(|px| px[3] == 255));
    }

    #[test]
    fn zero_sigma_is_identity() {
        let c = checker(16, 16);
        assert_eq!(blur(&c, 0.0).data, c.data);
    }

    #[test]
    fn blit_clips() {
        let mut dst = solid(4, 4, [0, 0, 0]);
        let src = solid(3, 3, [255, 255, 255]);
        blit(&mut dst, &src, -1, 2);
        let white = |x: i32, y: i32| dst.data[((y * 4 + x) * 4) as usize] == 255;
        assert!(white(0, 2) && white(1, 3) && !white(2, 2) && !white(0, 1));
    }
}

/// 実機の壁紙に対する確認 (手動実行: cargo test --release wallpaper_live -- --ignored --nocapture)。
#[cfg(test)]
mod live_tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    };

    #[test]
    #[ignore]
    fn wallpaper_live() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
            windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GetMonitorInfoW(
                MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY),
                &mut info,
            );
        }
        let t = std::time::Instant::now();
        let c = wallpaper_canvas(info.rcMonitor, BLUR_LIMIT).expect("canvas");
        let t1 = t.elapsed();
        for strength in [20, 50, 100] {
            let t2 = std::time::Instant::now();
            let b = blurred(&c.pixels, strength);
            println!(
                "strength={strength}: {}x{} in {:?}",
                b.width,
                b.height,
                t2.elapsed()
            );
        }
        println!(
            "key={} canvas={}x{} factor={:.3} load={:?}",
            c.key, c.pixels.width, c.pixels.height, c.factor, t1
        );
    }
}
