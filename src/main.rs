//! LaunchPanel3: 依存なしの単一 exe として再構築した常駐ランチャー (試作)。
//!
//! Win32 のウィンドウを Direct2D / DirectWrite で自前描画する。OS 連携 (トレイ・ホットキー・
//! アイコン・影・デスクトップ空白ダブルクリック) は LaunchPanel2 の lp_native から移植した。

#![windows_subsystem = "windows"]

mod app;
mod appearance;
mod backdrop;
mod color;
mod config;
mod dialog;
mod droptarget;
mod edit;
mod items;
mod layout;
mod log;
mod platform;
mod render;
mod services;
mod settings;
mod ui;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::app::App;
use crate::config::{LoadSource, Store};
use crate::services::SingleInstance;

fn main() {
    let instance = SingleInstance::acquire();
    if !instance.is_primary {
        // 既存インスタンスへ表示要求を送って終了する (常駐しない)
        instance.request_show();
        return;
    }

    // SAFETY: UI スレッドの OLE 初期化 (COM の STA に加え、ドラッグ＆ドロップの受け取りに必要)
    unsafe {
        let _ = OleInitialize(None);
    }

    // 設定は exe と同じフォルダーに置く (起動方法や作業フォルダーに左右されない)
    let store = Store::new(app_dir());
    let (config, source) = store.load();
    if source != LoadSource::Current {
        log::write(&format!("設定を読み込みました: {source:?}"));
    }

    let Some(hwnd) = create_window() else {
        log::write("ウィンドウを作成できませんでした。");
        return;
    };
    let app = match App::new(hwnd, store, config) {
        Ok(app) => Box::into_raw(app),
        Err(e) => {
            log::write(&format!("描画の初期化に失敗しました: {e}"));
            return;
        }
    };
    // SAFETY: app はウィンドウ破棄後のメッセージループ終了まで有効
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        app::set_main_hwnd(hwnd);
        instance.listen(hwnd, app::WM_APP_SHOW);
        (*app).start();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(app));
    }
}

/// 設定ファイルとログを置くフォルダー = exe のあるフォルダー。取得できなければ作業フォルダー。
pub fn app_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

fn create_window() -> Option<HWND> {
    // SAFETY: ウィンドウクラスの登録とウィンドウの生成
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("LaunchPanel3Window");
        let icon = LoadIconW(Some(instance.into()), windows::core::PCWSTR(1 as _)).ok();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: icon.unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok().unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // 枠なしのポップアップ。WS_EX_TOOLWINDOW でタスクバーに通常のボタンを出さない。
        // WS_SYSMENU は Alt+F4 (閉じる操作 = 非表示) のために残す
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_ACCEPTFILES,
            class,
            w!("LaunchPanel"),
            WS_POPUP | WS_SYSMENU | WS_CLIPCHILDREN,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA には main で設定した App のポインタだけが入る
    unsafe {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if !app.is_null() {
            if let Some(result) = (*app).handle(msg, wparam, lparam) {
                return result;
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
