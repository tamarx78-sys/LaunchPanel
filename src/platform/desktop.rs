//! デスクトップ空白ダブルクリックの検出。
//!
//! - フックスレッド: WH_MOUSE_LL で左ボタンの押下/移動/解放だけを見て、システム設定の
//!   ダブルクリック時間と許容移動量でダブルクリック候補を判定する。フック内では重い処理を
//!   一切せず、入力は必ず次のフックへ渡す (横取りしない)。注入された疑似入力は無視する。
//! - 判定スレッド: 候補の2点がどちらも Explorer の実デスクトップの空白 (アイコン・ラベル上でない) かを
//!   調べ、空白ならコールバックで通知する。クリック先は WindowFromPoint で求めるので、
//!   透明なオーバーレイがデスクトップを覆っていても実デスクトップを基準に判定できる。
//!
//! 2回目の押下ではなく、その後の解放で通知する。押下時点で前面化すると、続く解放で Explorer が
//! 前面に戻り、ランチャーがすぐ自動非表示になってしまうため。
//!
//! 空白判定はクリックのたびにウィンドウを探し直すので、Explorer 再起動後も再設定は要らない。

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows::Win32::UI::Controls::{LVHITTESTINFO, LVM_HITTEST};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

/// LVHT_ONITEMICON | LVHT_ONITEMLABEL | LVHT_ONITEMSTATEICON
const LVHT_ONITEM: u32 = 0x0002 | 0x0004 | 0x0008;

type ClickCallback = extern "system" fn(x: i32, y: i32);

static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

struct Running {
    hook_thread_id: u32,
    hook_thread: JoinHandle<()>,
    worker: JoinHandle<()>,
}

/// ダブルクリック候補 (1回目と2回目の押下位置)。
#[derive(Clone, Copy)]
struct Candidate {
    first: POINT,
    second: POINT,
}

/// フックスレッドだけが触る状態。
#[derive(Default)]
struct ClickState {
    /// 1回目の押下 (位置, 時刻)
    first: Option<(POINT, u32)>,
    /// 2回目の押下まで成立し、解放待ちの候補
    armed: Option<Candidate>,
    sender: Option<mpsc::Sender<Candidate>>,
}

thread_local! {
    static STATE: RefCell<ClickState> = RefCell::new(ClickState::default());
}

/// 監視を開始する。成功で 1、既に動作中なら 1、フック設定に失敗したら 0。
pub extern "system" fn lp_desktop_start(callback: ClickCallback) -> i32 {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    CALLBACK.store(callback as usize, Ordering::SeqCst);
    if running.is_some() {
        return 1;
    }

    let (tx, rx) = mpsc::channel::<Candidate>();
    let worker = std::thread::Builder::new()
        .name("lp-desktop-hittest".into())
        .spawn(move || {
            // フックの座標は物理ピクセルなので、判定も Per-Monitor V2 で行う (ホストの設定に依存しない)
            // SAFETY: このスレッドの DPI 設定の変更のみ
            unsafe {
                windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                    windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                );
            }
            for c in rx {
                if is_desktop_blank(c.first) && is_desktop_blank(c.second) {
                    notify(c.second);
                }
            }
        })
        .expect("failed to spawn hit-test thread");

    let (ready_tx, ready_rx) = mpsc::channel();
    let hook_thread = std::thread::Builder::new()
        .name("lp-desktop-hook".into())
        .spawn(move || run_hook(tx, ready_tx))
        .expect("failed to spawn hook thread");

    match ready_rx.recv() {
        Ok(Some(thread_id)) => {
            *running = Some(Running { hook_thread_id: thread_id, hook_thread, worker });
            1
        }
        _ => {
            let _ = hook_thread.join();
            let _ = worker.join(); // 送信側はフックスレッドと共に破棄済み
            0
        }
    }
}

/// 監視を停止する。
pub extern "system" fn lp_desktop_stop() {
    let Some(running) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    // SAFETY: 自分で作ったスレッドへの終了要求
    unsafe {
        let _ = PostThreadMessageW(running.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
    let _ = running.hook_thread.join();
    let _ = running.worker.join();
}

fn notify(pt: POINT) {
    let cb = CALLBACK.load(Ordering::SeqCst);
    if cb != 0 {
        // SAFETY: lp_desktop_start で受け取った有効な関数ポインタ
        let cb: ClickCallback = unsafe { std::mem::transmute(cb) };
        cb(pt.x, pt.y);
    }
}

fn run_hook(sender: mpsc::Sender<Candidate>, ready: mpsc::Sender<Option<u32>>) {
    STATE.with(|s| s.borrow_mut().sender = Some(sender));
    // SAFETY: フックはこのスレッドのメッセージループで呼ばれ、終了時に必ず解除する
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), Some(instance.into()), 0) {
            Ok(h) => h,
            Err(_) => {
                let _ = ready.send(None);
                return;
            }
        };
        let _ = ready.send(Some(GetCurrentThreadId()));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    STATE.with(|s| s.borrow_mut().sender = None);
}

/// 低レベルマウスフック。判定は比較だけにとどめ、入力は必ず次へ渡す。
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: WH_MOUSE_LL の lParam は MSLLHOOKSTRUCT
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let injected = info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0;
        if !injected {
            on_mouse(wparam.0 as u32, info.pt, info.time);
        }
    }
    // SAFETY: フックチェーンへの受け渡し
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn on_mouse(msg: u32, pt: POINT, time: u32) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        match msg {
            WM_LBUTTONDOWN => {
                let double = s.first.is_some_and(|(first, t)| {
                    // SAFETY: システム設定の参照のみ
                    let max = unsafe { GetDoubleClickTime() };
                    time.wrapping_sub(t) <= max && within_tolerance(first, pt)
                });
                if double {
                    let (first, _) = s.first.take().unwrap_or_default();
                    s.armed = Some(Candidate { first, second: pt });
                } else {
                    s.first = Some((pt, time));
                    s.armed = None;
                }
            }
            WM_MOUSEMOVE => {
                // クリック間に許容範囲を超えて動いたらダブルクリックとして扱わない
                if s.first.is_some_and(|(first, _)| !within_tolerance(first, pt)) {
                    s.first = None;
                }
            }
            WM_LBUTTONUP => {
                if let Some(candidate) = s.armed.take() {
                    if let Some(sender) = &s.sender {
                        let _ = sender.send(candidate);
                    }
                }
            }
            WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                s.first = None;
                s.armed = None;
            }
            _ => {}
        }
    });
}

/// システム設定のダブルクリック許容矩形 (クリック位置を中心とした幅・高さ) に収まるか。
fn within_tolerance(a: POINT, b: POINT) -> bool {
    // SAFETY: システム設定の参照のみ
    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDOUBLECLK), GetSystemMetrics(SM_CYDOUBLECLK)) };
    (a.x - b.x).abs() <= cx / 2 && (a.y - b.y).abs() <= cy / 2
}

// ───────────── 空白判定 (判定スレッド) ─────────────

/// `pt` (物理スクリーン座標) が Explorer の実デスクトップの空白部分か。
/// 判定できない場合は発火させない側 (false) に倒す。
fn is_desktop_blank(pt: POINT) -> bool {
    let Some(top) = top_level_at(pt) else { return false };
    let class = class_name(top);
    if class != "Progman" && class != "WorkerW" {
        return false;
    }
    let Some(listview) = desktop_listview(top) else {
        // デスクトップだがアイコン表示が無い (アイコン非表示など) → 全面が空白
        return true;
    };
    // SAFETY: ウィンドウ状態の参照のみ
    if !unsafe { IsWindowVisible(listview) }.as_bool() {
        return true;
    }
    matches!(hit_item(listview, pt), Some(false))
}

/// `pt` で実際にクリックを受け取るトップレベルウィンドウ。
///
/// WindowFromPoint はシステムのマウス入力と同じヒットテストを行うので、レイヤードウィンドウの
/// 透明な部分やクリック透過の窓を通り抜ける。窓の矩形と拡張スタイルからの推測では、
/// NVIDIA GeForce Overlay (WS_EX_LAYERED だが WS_EX_TRANSPARENT ではない全画面窓) のような
/// オーバーレイを最前面と誤判定してしまい、そのモニターでは一切発火しなくなる。
fn top_level_at(pt: POINT) -> Option<HWND> {
    // SAFETY: ウィンドウの参照のみ
    unsafe {
        let hwnd = WindowFromPoint(pt);
        if hwnd.is_invalid() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        Some(if root.is_invalid() { hwnd } else { root })
    }
}

/// デスクトップのアイコン一覧 (SHELLDLL_DefView 配下の SysListView32)。
/// DefView は Progman か WorkerW のどちらかにあり、壁紙の切替などで移動するので、
/// 当たった窓に無ければ全トップレベル窓から探す。
fn desktop_listview(top: HWND) -> Option<HWND> {
    // SAFETY: ウィンドウ検索のみ
    unsafe {
        let mut defview = FindWindowExW(Some(top), None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok();
        if defview.is_none() {
            let mut found: Option<HWND> = None;
            let _ = EnumWindows(Some(find_defview), LPARAM(&mut found as *mut _ as isize));
            defview = found;
        }
        FindWindowExW(Some(defview?), None, w!("SysListView32"), PCWSTR::null()).ok()
    }
}

unsafe extern "system" fn find_defview(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    // SAFETY: lparam は desktop_listview の Option<HWND> を指す
    unsafe {
        if let Ok(defview) = FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
            *(lparam.0 as *mut Option<HWND>) = Some(defview);
            return false.into();
        }
    }
    true.into()
}

/// デスクトップの ListView に LVM_HITTEST を送り、アイコンまたはラベル上なら Some(true)。
/// LVHITTESTINFO は Explorer のプロセス内に置く必要があるので、そこへ確保して読み書きする。
fn hit_item(listview: HWND, pt: POINT) -> Option<bool> {
    // SAFETY: Explorer プロセスへの小さな確保と読み書き。確保した領域とハンドルは必ず解放する
    unsafe {
        let mut client = pt;
        if !ScreenToClient(listview, &mut client).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(listview, Some(&mut pid));
        let process = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE, false, pid).ok()?;

        let size = size_of::<LVHITTESTINFO>();
        let remote = VirtualAllocEx(process, None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        let result = if remote.is_null() {
            None
        } else {
            let mut info = LVHITTESTINFO { pt: client, ..Default::default() };
            let mut hit = None;
            if WriteProcessMemory(process, remote, &info as *const _ as *const c_void, size, None).is_ok() {
                let mut ret = 0usize;
                let sent = SendMessageTimeoutW(
                    listview,
                    LVM_HITTEST,
                    WPARAM(0),
                    LPARAM(remote as isize),
                    SMTO_ABORTIFHUNG,
                    200,
                    Some(&mut ret),
                );
                if sent.0 != 0
                    && ReadProcessMemory(process, remote, &mut info as *mut _ as *mut c_void, size, None).is_ok()
                {
                    hit = Some(info.iItem >= 0 && info.flags.0 & LVHT_ONITEM != 0);
                }
            }
            let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
            hit
        };
        let _ = CloseHandle(process);
        result
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[(u32, i32, i32, u32)]) -> Vec<(i32, i32)> {
        let (tx, rx) = mpsc::channel();
        STATE.with(|s| *s.borrow_mut() = ClickState { sender: Some(tx), ..Default::default() });
        for &(msg, x, y, t) in events {
            on_mouse(msg, POINT { x, y }, t);
        }
        STATE.with(|s| s.borrow_mut().sender = None);
        rx.try_iter().map(|c| (c.second.x, c.second.y)).collect()
    }

    #[test]
    fn double_click_fires_on_second_release() {
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_LBUTTONUP, 100, 100, 1050),
            (WM_LBUTTONDOWN, 101, 100, 1150),
        ]);
        assert!(got.is_empty(), "押下時点では通知しない");
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_LBUTTONUP, 100, 100, 1050),
            (WM_LBUTTONDOWN, 101, 100, 1150),
            (WM_LBUTTONUP, 101, 100, 1200),
        ]);
        assert_eq!(got, vec![(101, 100)]);
    }

    #[test]
    fn slow_clicks_do_not_fire() {
        let slow = unsafe { GetDoubleClickTime() } + 50;
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_LBUTTONUP, 100, 100, 1050),
            (WM_LBUTTONDOWN, 100, 100, 1000 + slow),
            (WM_LBUTTONUP, 100, 100, 1050 + slow),
        ]);
        assert!(got.is_empty());
    }

    #[test]
    fn moving_between_clicks_cancels() {
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_LBUTTONUP, 100, 100, 1050),
            (WM_MOUSEMOVE, 200, 100, 1080), // 許容範囲外へ移動して戻る
            (WM_MOUSEMOVE, 100, 100, 1100),
            (WM_LBUTTONDOWN, 100, 100, 1150),
            (WM_LBUTTONUP, 100, 100, 1200),
        ]);
        assert!(got.is_empty());
    }

    #[test]
    fn triple_click_fires_once() {
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_LBUTTONUP, 100, 100, 1020),
            (WM_LBUTTONDOWN, 100, 100, 1100),
            (WM_LBUTTONUP, 100, 100, 1120),
            (WM_LBUTTONDOWN, 100, 100, 1200),
            (WM_LBUTTONUP, 100, 100, 1220),
        ]);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn right_click_between_cancels() {
        let got = feed(&[
            (WM_LBUTTONDOWN, 100, 100, 1000),
            (WM_RBUTTONDOWN, 100, 100, 1050),
            (WM_LBUTTONDOWN, 100, 100, 1100),
            (WM_LBUTTONUP, 100, 100, 1150),
        ]);
        assert!(got.is_empty());
    }
}

/// 実機のデスクトップに対する検証 (手動実行: cargo test --release -- --ignored --nocapture)。
#[cfg(test)]
mod live_tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Controls::{LVIR_BOUNDS, LVM_GETITEMCOUNT, LVM_GETITEMRECT};

    /// アイコン0番の外接矩形 (スクリーン座標) を Explorer のプロセス経由で取得する。
    fn item_rect(listview: HWND, index: usize) -> Option<RECT> {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(listview, Some(&mut pid));
            let process = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE, false, pid).ok()?;
            let size = size_of::<RECT>();
            let remote = VirtualAllocEx(process, None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            let mut rect = RECT { left: LVIR_BOUNDS as i32, ..Default::default() };
            WriteProcessMemory(process, remote, &rect as *const _ as *const c_void, size, None).ok()?;
            SendMessageW(listview, LVM_GETITEMRECT, Some(WPARAM(index)), Some(LPARAM(remote as isize)));
            ReadProcessMemory(process, remote, &mut rect as *mut _ as *mut c_void, size, None).ok()?;
            let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
            let _ = CloseHandle(process);
            let mut tl = POINT { x: rect.left, y: rect.top };
            let mut br = POINT { x: rect.right, y: rect.bottom };
            let _ = ClientToScreen(listview, &mut tl);
            let _ = ClientToScreen(listview, &mut br);
            Some(RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y })
        }
    }

    #[test]
    #[ignore]
    fn hit_test_against_real_desktop() {
        // アプリ本体と同じく Per-Monitor V2 で座標を扱う
        unsafe {
            windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let progman = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }.expect("Progman");
        let listview = desktop_listview(progman).expect("desktop listview");
        let count = unsafe { SendMessageW(listview, LVM_GETITEMCOUNT, None, None) }.0 as usize;
        println!("listview={listview:?} items={count}");
        assert!(count > 0, "デスクトップにアイコンが無いと検証できない");

        let r = item_rect(listview, 0).expect("item rect");
        let center = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
        let label = POINT { x: center.x, y: r.bottom - 4 };
        println!("item0 rect={r:?} center={:?} label={:?}", hit_item(listview, center), hit_item(listview, label));
        assert_eq!(hit_item(listview, center), Some(true), "アイコン上");
        assert_eq!(hit_item(listview, label), Some(true), "ラベル上");

        // 全アイコンの外側にある点を探して空白判定を確認する
        let rects: Vec<RECT> = (0..count).filter_map(|i| item_rect(listview, i)).collect();
        let mut lv = RECT::default();
        unsafe { GetWindowRect(listview, &mut lv).unwrap() };
        let blank = (0..40)
            .flat_map(|i| (0..40).map(move |j| POINT { x: lv.right - 30 - i * 40, y: lv.bottom - 30 - j * 40 }))
            .find(|p| rects.iter().all(|r| p.x < r.left - 10 || p.x > r.right + 10 || p.y < r.top - 10 || p.y > r.bottom + 10))
            .expect("blank point");
        println!("blank point={blank:?}");
        assert_eq!(hit_item(listview, blank), Some(false), "空白");
    }
}

#[cfg(test)]
mod diagnose {
    use super::*;
    use windows::Win32::Foundation::RECT;

    /// 指定点にある可視トップレベル窓を Z 順に列挙する (手動実行: LP_X / LP_Y 環境変数で点を指定)。
    #[test]
    #[ignore]
    fn windows_at_point() {
        unsafe {
            windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let x = std::env::var("LP_X").map(|v| v.parse().unwrap()).unwrap_or(1000);
        let y = std::env::var("LP_Y").map(|v| v.parse().unwrap()).unwrap_or(1000);
        let pt = POINT { x, y };
        println!("point={pt:?} top_level_at={:?} blank={}", top_level_at(pt).map(class_name), is_desktop_blank(pt));
        unsafe {
            let h = WindowFromPoint(pt);
            let root = GetAncestor(h, GA_ROOT);
            println!("WindowFromPoint={} root={}", class_name(h), class_name(root));
        }
        unsafe {
            let mut hwnd = GetTopWindow(None).unwrap();
            loop {
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let inside = pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
                if IsWindowVisible(hwnd).as_bool() && inside {
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                    let mut title = [0u16; 128];
                    let n = GetWindowTextW(hwnd, &mut title) as usize;
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    println!(
                        "{:?} class={} title={:?} ex=0x{ex:08X} rect={rect:?} pid={pid}",
                        hwnd.0,
                        class_name(hwnd),
                        String::from_utf16_lossy(&title[..n]),
                                        );
                    if class_name(hwnd) == "Progman" { break; }
                }
                match GetWindow(hwnd, GW_HWNDNEXT) { Ok(h) => hwnd = h, Err(_) => break }
            }
        }
    }
}
