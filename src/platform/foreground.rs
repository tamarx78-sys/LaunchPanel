//! ウィンドウの確実な前面化。
//!
//! 入力を受け取っていないプロセスの SetForegroundWindow はフォアグラウンドロックで拒否される
//! (デスクトップのダブルクリック直後やトレイからの復帰など)。Windows は「最後に入力を送った
//! プロセス」に前面化を許すので、移動量 0 のマウス入力を自分で送ってから前面化する。
//! この入力は何も動かさず、デスクトップのダブルクリック検出も注入入力として無視する。
//!
//! 前面スレッドと入力状態を共有する方法 (AttachThreadInput) は、共有相手が Explorer の
//! デスクトップのスレッドだと活性化の状態が食い違い、別のウィンドウへ移っても WM_ACTIVATE が
//! 届かず自動非表示が働かなくなるので、最後の手段としてだけ使う。

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_MOUSE, SendInput, SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
};

/// `hwnd` を前面化して入力フォーカスを与える。前面になれば 1。
///
/// # Safety
/// `hwnd` は呼び出しスレッドが所有する有効なウィンドウであること。
pub unsafe extern "system" fn lp_force_foreground(hwnd: isize) -> i32 {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: 呼び出し側の保証どおり
    unsafe {
        if try_set(hwnd) {
            crate::log::debug("前面化: 通常");
            return 1;
        }
        // 移動量 0 のマウス入力を送り、前面化の権利を得る
        let input = INPUT { r#type: INPUT_MOUSE, ..Default::default() };
        SendInput(&[input], size_of::<INPUT>() as i32);
        if try_set(hwnd) {
            crate::log::debug("前面化: 空のマウス入力で権利を得て成功");
            return 1;
        }
        crate::log::debug("前面化: 入力状態の共有で再試行");

        // 最後の手段: 前面スレッドと入力状態を一時的に共有する
        let foreground = GetForegroundWindow();
        let me = GetCurrentThreadId();
        let other = GetWindowThreadProcessId(foreground, None);
        let attached = other != 0 && other != me && AttachThreadInput(me, other, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(me, other, false);
        }
        if GetForegroundWindow() == hwnd {
            // 共有中の活性化は自スレッドに残らないことがあるので、今の権利でやり直す
            let _ = SetForegroundWindow(hwnd);
            let _ = SetActiveWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
        (GetForegroundWindow() == hwnd) as i32
    }
}

unsafe fn try_set(hwnd: HWND) -> bool {
    // SAFETY: 呼び出し側の保証どおり
    unsafe { SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd }
}
