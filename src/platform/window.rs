//! 本体とダイアログで共通のウィンドウ操作 (座標・DPI・カーソル・モニター・ファイル選択など)。

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_FROM_FLAGS, MONITORINFO, MonitorFromPoint, ScreenToClient,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragFinish, DragQueryFileW, FOS_PICKFOLDERS, FileOpenDialog, HDROP, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, LoadCursorW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetCursor,
    SetWindowPos,
};
use windows::core::PCWSTR;

use crate::platform::wide::to_wide;

/// DIP から物理ピクセルへの倍率。
pub fn scale(hwnd: HWND) -> f32 {
    // SAFETY: ウィンドウの DPI の参照のみ
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

/// マウスメッセージの lParam の位置をクライアント座標 (DIP) で。
pub fn point(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let s = scale(hwnd);
    (
        (lparam.0 & 0xFFFF) as i16 as f32 / s,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / s,
    )
}

/// 現在のカーソル位置をクライアント座標 (DIP) で。
pub fn cursor(hwnd: HWND) -> (f32, f32) {
    let mut pt = POINT::default();
    // SAFETY: カーソル位置の取得と変換
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = ScreenToClient(hwnd, &mut pt);
    }
    let s = scale(hwnd);
    (pt.x as f32 / s, pt.y as f32 / s)
}

/// WM_SIZE の lParam のクライアント領域の大きさ (物理ピクセル)。
pub fn client_size(lparam: LPARAM) -> (u32, u32) {
    (
        (lparam.0 & 0xFFFF) as u32,
        ((lparam.0 >> 16) & 0xFFFF) as u32,
    )
}

/// WM_DPICHANGED で、Windows の推奨する位置と大きさへ移す。
///
/// # Safety
/// `lparam` は WM_DPICHANGED の lParam (推奨矩形へのポインタ) であること。
pub unsafe fn apply_suggested_rect(hwnd: HWND, lparam: LPARAM) {
    // SAFETY: 呼び出し側の保証どおり
    unsafe {
        let r = &*(lparam.0 as *const RECT);
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// システムカーソル (IDC_*) を設定する。
pub fn set_cursor(id: PCWSTR) {
    // SAFETY: システムカーソルの読込と設定のみ
    unsafe {
        let _ = SetCursor(LoadCursorW(None, id).ok());
    }
}

/// ポインターが出た時に WM_MOUSELEAVE を受け取るよう要求する。要求できたら true。
pub fn track_leave(hwnd: HWND) -> bool {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: 自分のウィンドウのマウス追跡
    unsafe { TrackMouseEvent(&mut tme) }.is_ok()
}

/// ウィンドウのクラス名 (診断ログ・デスクトップの判定用)。無効なウィンドウなら空。
pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// モニターの作業領域 (物理座標)。
pub fn work_area(monitor: HMONITOR) -> RECT {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: モニター情報の取得
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    info.rcWork
}

/// `pt` (物理座標) のあるモニターの作業領域。
pub fn work_area_at(pt: POINT, flags: MONITOR_FROM_FLAGS) -> RECT {
    // SAFETY: モニターの検索のみ
    work_area(unsafe { MonitorFromPoint(pt, flags) })
}

/// 大きさ `w`×`h` のウィンドウを左上 (`x`, `y`) に置く時、`area` からはみ出さない左上。
/// `area` より大きければ左上を合わせる。
pub fn clamp_into(x: i32, y: i32, w: i32, h: i32, area: RECT) -> (i32, i32) {
    (
        x.min(area.right - w).max(area.left),
        y.min(area.bottom - h).max(area.top),
    )
}

/// 大きさを変えずに移動する (物理座標)。
pub fn move_to(hwnd: HWND, x: i32, y: i32) {
    // SAFETY: ウィンドウの移動のみ
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// WM_DROPFILES で受け取ったパスを読み、HDROP を解放する。
pub fn dropped_files(hdrop: HDROP) -> Vec<String> {
    let mut paths = Vec::new();
    // SAFETY: WM_DROPFILES の HDROP を読んで解放する
    unsafe {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            if n > 0 {
                paths.push(String::from_utf16_lossy(&buf[..n]));
            }
        }
        DragFinish(hdrop);
    }
    paths
}

/// ファイル (`folder` なら フォルダー) の選択ダイアログ。`filters` は (表示名, パターン)。
/// キャンセルなら None。
pub fn pick_file(
    owner: HWND,
    title: &str,
    filters: &[(&str, &str)],
    folder: bool,
) -> Option<String> {
    // 文字列は呼び出し中有効である必要があるので先に作っておく
    let title = to_wide(title);
    let wide: Vec<(Vec<u16>, Vec<u16>)> = filters
        .iter()
        .map(|(name, spec)| (to_wide(name), to_wide(spec)))
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = wide
        .iter()
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    // SAFETY: COM のファイル選択ダイアログ。文字列は呼び出し中有効
    unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        if folder && let Ok(options) = dlg.GetOptions() {
            let _ = dlg.SetOptions(options | FOS_PICKFOLDERS);
        }
        if !specs.is_empty() {
            let _ = dlg.SetFileTypes(&specs);
        }
        let _ = dlg.SetTitle(PCWSTR(title.as_ptr()));
        dlg.Show(Some(owner)).ok()?; // キャンセルはエラーになる
        let p = dlg
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        Some(s)
    }
}
