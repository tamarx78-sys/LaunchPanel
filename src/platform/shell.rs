//! タスクトレイアイコンとグローバルホットキー。
//!
//! 専用スレッドに非表示ウィンドウとメッセージループを持ち、UI スレッドの負荷と無関係に
//! トレイ操作・ホットキーを受け付ける。操作はコールバックで通知する (このスレッドから
//! 呼ばれるので、受け取り側で UI スレッドへ転送すること)。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::platform::wide::to_wide;

/// トレイ・ホットキーからの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// トレイアイコンのクリック、またはメニューの「表示」
    Show,
    Settings,
    Exit,
    Hotkey,
}

impl ShellEvent {
    const ALL: [Self; 4] = [Self::Show, Self::Settings, Self::Exit, Self::Hotkey];

    /// ウィンドウメッセージで受け渡すための番号。
    pub fn to_wparam(self) -> usize {
        self as usize
    }

    pub fn from_wparam(w: usize) -> Option<Self> {
        Self::ALL.get(w).copied()
    }
}

const TOOLTIP: &str = "LaunchPanel";
/// メニューの項目 (コマンド ID は並び順 + 1)
const MENU: [(&str, ShellEvent); 3] = [
    ("表示", ShellEvent::Show),
    ("設定", ShellEvent::Settings),
    ("終了", ShellEvent::Exit),
];

const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const HOTKEY_ID: i32 = 1;

static CALLBACK: OnceLock<fn(ShellEvent)> = OnceLock::new();
static HWND_VALUE: AtomicIsize = AtomicIsize::new(0);
static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// ウィンドウスレッドだけが触る状態。
struct TrayState {
    tooltip: Vec<u16>,
    labels: Vec<Vec<u16>>,
    icon: HICON,
    taskbar_created: u32,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<TrayState>> = const { std::cell::RefCell::new(None) };
}

/// トレイとホットキーのスレッドを開始する。`modifiers` は MOD_ALT=1, MOD_CONTROL=2,
/// MOD_SHIFT=4, MOD_WIN=8 の組合せ。ホットキーを登録できたら true (登録できなくても
/// トレイは動作を続ける)。既に動作中なら何もしない。
pub fn start(on_event: fn(ShellEvent), modifiers: u32, key: char) -> bool {
    let mut thread = THREAD.lock().unwrap_or_else(|e| e.into_inner());
    if thread.is_some() {
        return true;
    }
    let _ = CALLBACK.set(on_event);
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("lp-shell".into())
        .spawn(move || run(modifiers, key as u32, tx))
        .expect("failed to spawn shell thread");
    let hotkey_ok = rx.recv().unwrap_or(false);
    *thread = Some(handle);
    hotkey_ok
}

/// トレイアイコンを削除し、スレッドを終了する。
pub fn stop() {
    let handle = THREAD.lock().unwrap_or_else(|e| e.into_inner()).take();
    let hwnd = HWND_VALUE.load(Ordering::SeqCst);
    if hwnd != 0 {
        // SAFETY: 自スレッドで作った窓へのメッセージ送信のみ
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

fn notify(event: ShellEvent) {
    if let Some(cb) = CALLBACK.get() {
        cb(event);
    }
}

fn run(modifiers: u32, vk: u32, ready: mpsc::Sender<bool>) {
    // SAFETY: Win32 API 呼び出し。ハンドルはすべてこのスレッド内で生成・破棄する
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("LaunchPanelShellWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let icon = load_icon();
        STATE.with(|s| {
            *s.borrow_mut() = Some(TrayState {
                tooltip: to_wide(TOOLTIP),
                labels: MENU.iter().map(|(l, _)| to_wide(l)).collect(),
                icon,
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            })
        });

        // TaskbarCreated (Explorer 再起動) のブロードキャストを受けるため、
        // メッセージ専用ではなく表示しないトップレベル窓にする
        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name,
            w!("LaunchPanel"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                crate::log::write("トレイ用のウィンドウを作成できませんでした。");
                let _ = ready.send(false);
                return;
            }
        };
        HWND_VALUE.store(hwnd.0 as isize, Ordering::SeqCst);

        if !add_tray_icon(hwnd) {
            crate::log::write("トレイアイコンを追加できませんでした。");
        }
        let hotkey_ok = RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
            vk,
        )
        .is_ok();
        let _ = ready.send(hotkey_ok);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        HWND_VALUE.store(0, Ordering::SeqCst);
    }
}

/// exe に埋め込んだアプリのアイコン (リソース ID 1) の小サイズ。
unsafe fn load_icon() -> HICON {
    // SAFETY: Win32 API 呼び出し
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        match LoadImageW(
            Some(instance.into()),
            PCWSTR(1 as _),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        ) {
            Ok(h) => HICON(h.0),
            Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        }
    }
}

fn notify_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

unsafe fn add_tray_icon(hwnd: HWND) -> bool {
    let mut nid = notify_icon_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    STATE.with(|s| {
        if let Some(state) = s.borrow().as_ref() {
            nid.hIcon = state.icon;
            let n = state.tooltip.len().min(nid.szTip.len() - 1);
            nid.szTip[..n].copy_from_slice(&state.tooltip[..n]);
        }
    });
    // SAFETY: nid は有効な構造体
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() }
}

unsafe fn show_menu(hwnd: HWND) {
    // SAFETY: Win32 API 呼び出し。メニューは関数内で生成・破棄する
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        STATE.with(|s| {
            if let Some(state) = s.borrow().as_ref() {
                for (i, label) in state.labels.iter().enumerate() {
                    let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(label.as_ptr()));
                }
            }
        });
        let _ = SetMenuDefaultItem(menu, 1, 0);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // メニュー外クリックで閉じるよう前面化が必要 (KB135788)
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if let Some((_, event)) = (cmd.0 as usize).checked_sub(1).and_then(|i| MENU.get(i)) {
            notify(*event);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: ウィンドウプロシージャ内の Win32 API 呼び出し
    unsafe {
        match msg {
            WM_TRAY => {
                match lparam.0 as u32 {
                    WM_LBUTTONUP => notify(ShellEvent::Show),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HOTKEY if wparam.0 as i32 == HOTKEY_ID => {
                notify(ShellEvent::Hotkey);
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                let _ = Shell_NotifyIconW(NIM_DELETE, &notify_icon_data(hwnd));
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                let taskbar_created =
                    STATE.with(|s| s.borrow().as_ref().map_or(0, |st| st.taskbar_created));
                if taskbar_created != 0 && msg == taskbar_created {
                    // Explorer 再起動後にトレイアイコンを再登録する
                    add_tray_icon(hwnd);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }
}
