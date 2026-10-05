//! 多重起動防止、アイテムの起動、アイコンの非同期読込。

use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, PostMessageW, SW_SHOWNORMAL,
};
use windows::core::{PCWSTR, w};

use crate::platform::icon::{self, Pixels};
use crate::platform::wide::to_wide;

// ───────────── 多重起動防止 ─────────────

// v1 (egui 版) と同じ名前にする。どちらかが動いていれば、もう一方は起動せずに動いている方を表示する
#[cfg(debug_assertions)]
const MUTEX_NAME: &str = r"Local\LaunchPanel_Debug_Mutex";
#[cfg(debug_assertions)]
const SHOW_EVENT_NAME: &str = r"Local\LaunchPanel_Debug_Show";
#[cfg(not(debug_assertions))]
const MUTEX_NAME: &str = r"Local\LaunchPanel_Mutex";
#[cfg(not(debug_assertions))]
const SHOW_EVENT_NAME: &str = r"Local\LaunchPanel_Show";

/// 同一ユーザーセッション内の多重起動防止。後発プロセスは既存側へ表示要求を送って終わる。
/// デバッグ版は別名なので通常版と共存できる。
pub struct SingleInstance {
    show_event: HANDLE,
    pub is_primary: bool,
}

impl SingleInstance {
    pub fn acquire() -> Self {
        // SAFETY: 名前付きカーネルオブジェクトの生成。プロセス終了まで保持する
        unsafe {
            // イベントを先に作る (後発が開く前に既存側が待機できるように)
            let event_name = to_wide(SHOW_EVENT_NAME);
            let show_event =
                CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())).unwrap_or_default();
            let mutex_name = to_wide(MUTEX_NAME);
            let _mutex = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr()));
            let is_primary = GetLastError() != ERROR_ALREADY_EXISTS;
            Self {
                show_event,
                is_primary,
            }
        }
    }

    /// 後発プロセス側: 前面化の権利を既存側へ譲ってから表示要求を送る。
    pub fn request_show(&self) {
        // SAFETY: イベントの通知のみ
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = SetEvent(self.show_event);
        }
    }

    /// 既存側: 表示要求を受けるたびに `message` を `hwnd` へ送る (待機スレッド上)。
    pub fn listen(&self, hwnd: HWND, message: u32) {
        let event = self.show_event.0 as isize;
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("lp-single-instance".into())
            .spawn(move || {
                loop {
                    // SAFETY: プロセス終了まで有効なイベントを待つ
                    unsafe {
                        WaitForSingleObject(HANDLE(event as *mut _), INFINITE);
                        let _ =
                            PostMessageW(Some(HWND(hwnd as *mut _)), message, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .ok();
    }
}

// ───────────── 起動 ─────────────

/// OS の標準関連付けに従って開く。失敗したら false (例外的な状態でも落とさない)。
pub fn launch(hwnd: HWND, path: &str) -> bool {
    let target = expand_env(path.trim());
    let file = to_wide(&target);
    // ローカルのファイルは自身のフォルダーを作業フォルダーにする
    let dir = std::path::Path::new(&target)
        .is_file()
        .then(|| {
            std::path::Path::new(&target)
                .parent()
                .map(|p| to_wide(&p.to_string_lossy()))
        })
        .flatten();
    // SAFETY: 文字列は呼び出し中有効
    let result = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            SW_SHOWNORMAL,
        )
    };
    let ok = result.0 as isize > 32;
    if !ok {
        crate::log::write(&format!(
            "起動に失敗しました: {target} (コード {})",
            result.0 as isize
        ));
    }
    ok
}

fn expand_env(path: &str) -> String {
    if !path.contains('%') {
        return path.to_owned();
    }
    let src = to_wide(path);
    let mut buf = vec![0u16; 32768];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        path.to_owned()
    } else {
        String::from_utf16_lossy(&buf[..n - 1])
    }
}

// ───────────── アイコン ─────────────

/// アイコンの抽出は重いので専用 STA スレッドで1件ずつ行い、完了を UI スレッドへ通知する。
pub struct IconLoader {
    requests: Sender<(u64, String)>,
    results: Receiver<(u64, Option<Pixels>)>,
}

impl IconLoader {
    /// 完了するたびに `message` を `hwnd` へ送る。`size` は取得する画素サイズ。
    pub fn new(hwnd: HWND, message: u32, size: i32) -> Self {
        let (req_tx, req_rx) = channel::<(u64, String)>();
        let (res_tx, res_rx) = channel();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("lp-icon".into())
            .spawn(move || {
                // SAFETY: このスレッドの COM 初期化 (シェル拡張のため STA)
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                for (id, path) in req_rx {
                    let pixels = icon::load(&path, size);
                    if res_tx.send((id, pixels)).is_err() {
                        break;
                    }
                    // SAFETY: メッセージの送信のみ
                    unsafe {
                        let _ =
                            PostMessageW(Some(HWND(hwnd as *mut _)), message, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .ok();
        Self {
            requests: req_tx,
            results: res_rx,
        }
    }

    pub fn request(&self, id: u64, path: &str) {
        let _ = self.requests.send((id, path.to_owned()));
    }

    /// 完了した結果を取り出す。
    pub fn drain(&self) -> Vec<(u64, Option<Pixels>)> {
        self.results.try_iter().collect()
    }
}
