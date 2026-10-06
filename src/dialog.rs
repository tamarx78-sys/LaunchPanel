//! メインウィンドウに所有されるダイアログウィンドウの共通部分。
//!
//! メインウィンドウは最小 200px まで縮められるので、設定画面などは別のトップレベルウィンドウとして
//! 開く (本体からはみ出す大きさにできる)。開いている間は本体を無効化し、閉じる時は本体を有効に
//! 戻してから破棄するので、活性化は本体へ戻る。中身は各ダイアログが Direct2D で描く。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::platform::wide::to_wide;
use crate::platform::window;

/// ダイアログの中身。ウィンドウメッセージを受け取り、処理したら Some を返す。
pub trait Dialog {
    fn handle(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT>;
}

const CLASS: PCWSTR = w!("LaunchPanelDialog");

/// `owner` に所有されるダイアログを開く。`width`×`height` はクライアント領域の DIP。
/// 画面の作業領域の 9 割を超えないよう縮め、本体の中央に重ねて置く。
pub fn open(
    owner: HWND,
    title: &str,
    width: f64,
    height: f64,
    make: impl FnOnce(HWND) -> Box<dyn Dialog>,
) -> Option<HWND> {
    // SAFETY: ウィンドウクラスの登録とウィンドウの生成・配置
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // 2回目以降は登録済みで失敗するが問題ない

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let ex_style = WS_EX_DLGMODALFRAME | WS_EX_ACCEPTFILES;
        let title = to_wide(title);
        let hwnd = CreateWindowExW(
            ex_style,
            CLASS,
            PCWSTR(title.as_ptr()),
            style,
            0,
            0,
            100,
            100,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let dark = 1i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            4,
        );

        // 本体のいるモニターの作業領域に収まる大きさで、本体の中央に重ねる
        let dpi = GetDpiForWindow(owner).max(96);
        let scale = dpi as f64 / 96.0;
        let work = window::work_area(MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST));
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (width * scale) as i32,
            bottom: (height * scale) as i32,
        };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, ex_style, dpi);
        let w = (rc.right - rc.left).min((work.right - work.left) * 9 / 10);
        let h = (rc.bottom - rc.top).min((work.bottom - work.top) * 9 / 10);
        let mut orc = RECT::default();
        let _ = GetWindowRect(owner, &mut orc);
        let x = (orc.left + (orc.right - orc.left - w) / 2)
            .clamp(work.left, (work.right - w).max(work.left));
        let y = (orc.top + (orc.bottom - orc.top - h) / 2)
            .clamp(work.top, (work.bottom - h).max(work.top));
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);

        let content: Box<Box<dyn Dialog>> = Box::new(make(hwnd));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(content) as isize);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        Some(hwnd)
    }
}

/// ダイアログを閉じる。本体を先に有効へ戻すので、活性化は本体へ移る。
pub fn close(hwnd: HWND) {
    // SAFETY: 自分で開いたダイアログの破棄
    unsafe {
        if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
            let _ = EnableWindow(owner, true);
        }
        let _ = DestroyWindow(hwnd);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA には open で設定した Box<Box<dyn Dialog>> だけが入る
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Box<dyn Dialog>;
        if msg == WM_NCDESTROY && !ptr.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
        } else if !ptr.is_null()
            && let Some(result) = (*ptr).handle(hwnd, msg, wparam, lparam)
        {
            return result;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
