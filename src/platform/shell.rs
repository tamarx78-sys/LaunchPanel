//! タスクトレイアイコンとグローバルホットキー。
//!
//! 専用スレッドに非表示ウィンドウとメッセージループを持ち、UI スレッドの負荷と無関係に
//! トレイ操作・ホットキーを受け付ける。イベントはコールバックで C# 側へ通知する
//! (C# 側で UI スレッドへ転送すること)。

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc;
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

use crate::platform::wide::{from_ptr, to_wide};

/// C# へ通知するイベント。
#[repr(u32)]
#[derive(Clone, Copy)]
enum ShellEvent {
    Show = 1,
    Settings = 2,
    Exit = 3,
    Hotkey = 4,
}

/// `lp_shell_start` の戻り値ビット。
const STATUS_TRAY_OK: u32 = 1;
const STATUS_HOTKEY_OK: u32 = 2;
const STATUS_ALREADY_RUNNING: u32 = 0x8000_0000;

type EventCallback = extern "system" fn(event: u32);

const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const HOTKEY_ID: i32 = 1;
const CMD_SHOW: usize = 1;
const CMD_SETTINGS: usize = 2;
const CMD_EXIT: usize = 3;

static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static HWND_VALUE: AtomicIsize = AtomicIsize::new(0);
static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// ウィンドウスレッドだけが触る状態。
struct TrayState {
    tooltip: Vec<u16>,
    labels: [Vec<u16>; 3],
    icon: HICON,
    taskbar_created: u32,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<TrayState>> = const { std::cell::RefCell::new(None) };
}

/// トレイとホットキーのスレッドを開始する。
///
/// - `labels`: メニュー「表示」「設定」「終了」の文字列
/// - `icon_path`: .ico ファイルのパス (読めなければ既定アイコン)
/// - `modifiers`: MOD_ALT=1, MOD_CONTROL=2, MOD_SHIFT=4, MOD_WIN=8 の組合せ
/// - `vk`: 仮想キーコード。0 ならホットキーを登録しない
///
/// 戻り値は STATUS_* ビットの組合せ。ホットキー登録に失敗してもトレイは動作を続ける。
///
/// # Safety
/// 文字列引数は NUL 終端の UTF-16 か null。`callback` はプロセス終了まで有効であること。
pub unsafe extern "system" fn lp_shell_start(
    callback: EventCallback,
    tooltip: *const u16,
    show_label: *const u16,
    settings_label: *const u16,
    exit_label: *const u16,
    icon_path: *const u16,
    modifiers: u32,
    vk: u32,
) -> u32 {
    let mut thread = THREAD.lock().unwrap_or_else(|e| e.into_inner());
    if thread.is_some() {
        return STATUS_ALREADY_RUNNING;
    }
    CALLBACK.store(callback as usize, Ordering::SeqCst);

    // SAFETY: 呼び出し側の保証どおり
    let (tooltip, labels, icon_path) = unsafe {
        (
            from_ptr(tooltip),
            [from_ptr(show_label), from_ptr(settings_label), from_ptr(exit_label)],
            from_ptr(icon_path),
        )
    };

    let (tx, rx) = mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("lp-shell".into())
        .spawn(move || run(tooltip, labels, icon_path, modifiers, vk, tx))
        .expect("failed to spawn shell thread");
    let status = rx.recv().unwrap_or(0);
    *thread = Some(handle);
    status
}

/// トレイアイコンを削除し、スレッドを終了する。
pub extern "system" fn lp_shell_stop() {
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
    let cb = CALLBACK.load(Ordering::SeqCst);
    if cb != 0 {
        // SAFETY: lp_shell_start で受け取った有効な関数ポインタ
        let cb: EventCallback = unsafe { std::mem::transmute(cb) };
        cb(event as u32);
    }
}

fn run(
    tooltip: String,
    labels: [String; 3],
    icon_path: String,
    modifiers: u32,
    vk: u32,
    ready: mpsc::Sender<u32>,
) {
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

        let icon = load_icon(&icon_path);
        STATE.with(|s| {
            *s.borrow_mut() = Some(TrayState {
                tooltip: to_wide(&tooltip),
                labels: labels.map(|l| to_wide(&l)),
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
                let _ = ready.send(0);
                return;
            }
        };
        HWND_VALUE.store(hwnd.0 as isize, Ordering::SeqCst);

        let mut status = 0;
        if add_tray_icon(hwnd) {
            status |= STATUS_TRAY_OK;
        }
        if vk != 0
            && RegisterHotKey(
                Some(hwnd),
                HOTKEY_ID,
                HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
                vk,
            )
            .is_ok()
        {
            status |= STATUS_HOTKEY_OK;
        }
        let _ = ready.send(status);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        HWND_VALUE.store(0, Ordering::SeqCst);
    }
}

unsafe fn load_icon(path: &str) -> HICON {
    // SAFETY: Win32 API 呼び出し
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        if path.is_empty() {
            // exe に埋め込んだアプリのアイコン (リソース ID 1)
            let instance = GetModuleHandleW(None).unwrap_or_default();
            if let Ok(h) = LoadImageW(Some(instance.into()), PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                return HICON(h.0);
            }
        } else {
            let wide = to_wide(path);
            if let Ok(h) = LoadImageW(
                None,
                PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                size,
                size,
                LR_LOADFROMFILE,
            ) {
                return HICON(h.0);
            }
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
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
        let _ = SetMenuDefaultItem(menu, CMD_SHOW as u32, 0);
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
        match cmd.0 as usize {
            CMD_SHOW => notify(ShellEvent::Show),
            CMD_SETTINGS => notify(ShellEvent::Settings),
            CMD_EXIT => notify(ShellEvent::Exit),
            _ => {}
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
                let taskbar_created = STATE.with(|s| s.borrow().as_ref().map_or(0, |st| st.taskbar_created));
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
