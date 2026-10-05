//! 枠なしウィンドウの影。
//!
//! WinUI 3 は DWM のフレーム設定を自前で管理しており、DWM 標準の影を枠なし窓へ確実に付けられない。
//! そこで影だけを描く別ウィンドウ (レイヤード・クリック透過・非アクティブ化・タスクバー非表示) を
//! 対象ウィンドウの Z 順の直下に置き、対象をサブクラス化して移動・リサイズ・表示/非表示へ追従させる。
//!
//! 影はガウスぼかしした矩形で、ぼかした矩形は横方向と縦方向のプロファイルの積に分解できるので、
//! リサイズ中に描き直しても軽い。対象ウィンドウの真下は透明にし、アクリル背景を暗くしない。

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const SUBCLASS_ID: usize = 0x4C50_5348; // "LPSH"

/// 96 DPI での影の広がり (px) と下方向へのずれ (px)。
const RADIUS_DIP: f64 = 14.0;
const OFFSET_Y_DIP: f64 = 3.0;

thread_local! {
    /// 影を付けているウィンドウ → 状態。対象ウィンドウの UI スレッドだけが触る
    static ATTACHED: std::cell::RefCell<std::collections::HashMap<isize, *mut ShadowState>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

struct ShadowState {
    shadow: HWND,
    /// 濃さ (0～100%)
    opacity: i32,
    /// 最後に描いた (幅, 高さ, DPI, 濃さ)。変わった時だけ描き直す
    rendered: (i32, i32, u32, i32),
}

/// `hwnd` の影を表示する (enabled != 0) か外す。`opacity` は濃さ (0～100%)。成功で 1。
///
/// # Safety
/// `hwnd` は呼び出しスレッドが所有する有効なウィンドウであること。
pub unsafe extern "system" fn lp_set_window_shadow(hwnd: isize, enabled: i32, opacity: i32) -> i32 {
    let opacity = opacity.clamp(0, 100);
    let target = HWND(hwnd as *mut _);
    // SAFETY: 呼び出し側の保証どおり。状態はサブクラスの参照データとして所有し、解除時に破棄する
    unsafe {
        // GetWindowSubclass は comctl32 v5 (マニフェスト無しで読まれる版) に名前で公開されておらず、
        // 参照すると DLL 自体が読み込めなくなるので、状態は自前の表で管理する
        let existing = ATTACHED.with(|m| m.borrow().get(&(target.0 as isize)).copied());
        if enabled == 0 {
            if let Some(state) = existing {
                detach(target, state);
            }
            return 1;
        }
        if let Some(state) = existing {
            (*state).opacity = opacity;
            update(target, &mut *state);
            return 1;
        }

        let Some(shadow) = create_shadow_window() else {
            return 0;
        };
        let state = Box::into_raw(Box::new(ShadowState {
            shadow,
            opacity,
            rendered: (0, 0, 0, -1),
        }));
        if !SetWindowSubclass(target, Some(subclass_proc), SUBCLASS_ID, state as usize).as_bool() {
            let _ = DestroyWindow(shadow);
            drop(Box::from_raw(state));
            return 0;
        }
        ATTACHED.with(|m| m.borrow_mut().insert(target.0 as isize, state));
        update(target, &mut *state);
    }
    1
}

unsafe fn detach(target: HWND, state: *mut ShadowState) {
    // SAFETY: state は lp_set_window_shadow で確保したもの
    unsafe {
        ATTACHED.with(|m| m.borrow_mut().remove(&(target.0 as isize)));
        let _ = RemoveWindowSubclass(target, Some(subclass_proc), SUBCLASS_ID);
        let state = Box::from_raw(state);
        let _ = DestroyWindow(state.shadow);
    }
}

unsafe fn create_shadow_window() -> Option<HWND> {
    // SAFETY: Win32 API 呼び出し
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("LaunchPanelShadow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(shadow_wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc); // 2回目以降は既に登録済みで失敗するが問題ない
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: data は lp_set_window_shadow で確保した ShadowState
    unsafe {
        let result = DefSubclassProc(hwnd, msg, wparam, lparam);
        match msg {
            WM_WINDOWPOSCHANGED | WM_SHOWWINDOW | WM_DPICHANGED | WM_ACTIVATE => {
                update(hwnd, &mut *(data as *mut ShadowState));
            }
            WM_NCDESTROY => detach(hwnd, data as *mut ShadowState),
            _ => {}
        }
        result
    }
}

/// 対象ウィンドウの状態に影を合わせる。
unsafe fn update(target: HWND, state: &mut ShadowState) {
    // SAFETY: Win32 API 呼び出し
    unsafe {
        if !IsWindowVisible(target).as_bool() || IsIconic(target).as_bool() {
            let _ = ShowWindow(state.shadow, SW_HIDE);
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(target, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(target).max(96);
        let scale = dpi as f64 / 96.0;
        let radius = (RADIUS_DIP * scale).round() as i32;
        let offset_y = (OFFSET_Y_DIP * scale).round() as i32;
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let pos = POINT {
            x: rect.left - radius,
            y: rect.top - radius + offset_y,
        };
        let size = SIZE {
            cx: w + radius * 2,
            cy: h + radius * 2,
        };

        let key = (w, h, dpi, state.opacity);
        if state.rendered != key && render(state.shadow, pos, size, radius, offset_y, state.opacity)
        {
            state.rendered = key;
        }
        // 位置を合わせ、Z 順を対象の直下にする
        let _ = SetWindowPos(
            state.shadow,
            Some(target),
            pos.x,
            pos.y,
            size.cx,
            size.cy,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSIZE,
        );
    }
}

/// 影のビットマップを作ってレイヤードウィンドウへ反映する。
unsafe fn render(
    shadow: HWND,
    pos: POINT,
    size: SIZE,
    radius: i32,
    offset_y: i32,
    opacity: i32,
) -> bool {
    let pixels = shadow_pixels(
        size.cx,
        size.cy,
        radius,
        offset_y,
        opacity as f64 / 100.0 * 255.0,
    );
    // SAFETY: GDI 資源は関数内で生成・解放する
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.cx,
                biHeight: -size.cy,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let ok = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) if !bits.is_null() => {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
                let old = SelectObject(mem, HGDIOBJ(bitmap.0));
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let src = POINT::default();
                let ok = UpdateLayeredWindow(
                    shadow,
                    Some(screen),
                    Some(&pos),
                    Some(&size),
                    Some(mem),
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
                .is_ok();
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                ok
            }
            _ => false,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        ok
    }
}

/// 黒い影の BGRA (乗算済みアルファ)。`max_alpha` は最も濃い部分のアルファ (0～255)。
/// 対象ウィンドウの真下は透明。
fn shadow_pixels(width: i32, height: i32, radius: i32, offset_y: i32, max_alpha: f64) -> Vec<u8> {
    let sigma = (radius as f64 / 2.5).max(0.5);
    // ウィンドウ本体の範囲 (影ビットマップ内の座標)
    // 影の矩形は本体を offset_y だけ下へずらしたもの。ビットマップは影の矩形の周囲に radius の余白を持つ
    let (x0, x1) = (radius, width - radius);
    let gx = profile(width, x0, x1, sigma);
    let gy = profile(height, radius, height - radius, sigma);
    // 本体の真下 (影の矩形を offset_y だけ上へ戻した範囲) は透明にする
    let (cy0, cy1) = (radius - offset_y, height - radius - offset_y);

    let mut data = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            if x >= x0 && x < x1 && y >= cy0 && y < cy1 {
                continue;
            }
            let a = (max_alpha * gx[x as usize] * gy[y as usize])
                .round()
                .clamp(0.0, 255.0) as u8;
            data[((y * width + x) * 4 + 3) as usize] = a; // 黒なので色成分は 0 のまま (乗算済み)
        }
    }
    data
}

/// 区間 [start, end) をガウスぼかしした 1 次元プロファイル (0～1)。
fn profile(len: i32, start: i32, end: i32, sigma: f64) -> Vec<f64> {
    let k = std::f64::consts::SQRT_2 * sigma;
    (0..len)
        .map(|i| {
            let c = i as f64 + 0.5;
            0.5 * (erf((c - start as f64) / k) - erf((c - end as f64) / k))
        })
        .collect()
}

/// 誤差関数の近似 (Abramowitz-Stegun 7.1.26、誤差 1.5e-7 以下)。
fn erf(x: f64) -> f64 {
    let sign = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_one_inside_and_fades_outside() {
        let p = profile(100, 20, 80, 4.0);
        assert!(p[50] > 0.999);
        assert!(p[0] < 0.001 && p[99] < 0.001);
        assert!((p[20] - 0.5).abs() < 0.1, "端でおよそ半分");
        assert!(p[10] < p[15] && p[15] < p[20], "外へ向かって単調に薄くなる");
    }

    #[test]
    fn body_area_is_transparent_and_edges_have_shadow() {
        let (w, h, r, oy) = (140, 120, 20, 3);
        let px = shadow_pixels(w, h, r, oy, 110.0);
        let alpha = |x: i32, y: i32| px[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(w / 2, h / 2), 0, "本体の真下は透明");
        assert!(alpha(r - 3, h / 2) > 0, "左辺のすぐ外に影");
        assert!(
            alpha(w / 2, h - r + 1) > alpha(w / 2, r - oy - 5),
            "下側の方が濃い (下へずらしている)"
        );
        assert_eq!(alpha(0, 0), 0);
        assert!(
            px.chunks_exact(4)
                .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0),
            "黒の乗算済み"
        );
    }
}

unsafe extern "system" fn shadow_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // クリック透過に加え、ヒットテストでも素通りさせる
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // SAFETY: 既定処理への委譲
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod opacity_tests {
    use super::*;

    #[test]
    fn opacity_scales_alpha() {
        let (w, h, r) = (100, 80, 16);
        let peak = |a: f64| {
            shadow_pixels(w, h, r, 0, a)
                .chunks_exact(4)
                .map(|p| p[3])
                .max()
                .unwrap()
        };
        assert_eq!(peak(0.0), 0, "0% なら完全に透明");
        assert!(
            peak(255.0) > peak(102.0) && peak(102.0) > 0,
            "濃さに比例して濃くなる"
        );
    }
}
