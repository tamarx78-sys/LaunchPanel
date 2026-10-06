//! ウィンドウの確実な前面化。
//!
//! 入力を受け取っていないプロセスの SetForegroundWindow はフォアグラウンドロックで拒否される
//! (デスクトップのダブルクリック直後やトレイからの復帰など)。Windows は「最後に入力を送った
//! プロセス」に前面化を許すので、移動量 0 のマウス入力を自分で送ってから前面化する。
//! この入力は何も動かさず、デスクトップのダブルクリック検出も注入入力として無視する。
//!
//! 他スレッドと入力状態を共有する方法 (AttachThreadInput) は使わない。Explorer のデスクトップ
//! 相手だと活性化の状態が食い違って自動非表示が壊れるうえ、ウイルス対策ソフトに不審な挙動と
//! 見なされやすいため。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_MOUSE, SendInput, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, SetForegroundWindow,
};

/// `hwnd` を前面化して入力フォーカスを与える。前面になれば 1。
///
/// # Safety
/// `hwnd` は呼び出しスレッドが所有する有効なウィンドウであること。
pub unsafe extern "system" fn lp_force_foreground(hwnd: isize) -> i32 {
    let hwnd = HWND(hwnd as *mut _);
    if simulate_failure() {
        return 0;
    }
    // SAFETY: 呼び出し側の保証どおり
    unsafe {
        if try_set(hwnd) {
            crate::log::debug("前面化: 通常");
            return 1;
        }
        // 移動量 0 のマウス入力を送り、前面化の権利を得る
        let input = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
        SendInput(&[input], size_of::<INPUT>() as i32);
        let _ = BringWindowToTop(hwnd);
        let ok = try_set(hwnd);
        if ok {
            let _ = SetFocus(Some(hwnd));
        }
        crate::log::debug(if ok {
            "前面化: 空のマウス入力で権利を得て成功"
        } else {
            "前面化: 失敗"
        });
        ok as i32
    }
}

/// デバッグ版のテスト用 (LAUNCHPANEL_TEST_FG_FAIL=1): 前面化できない状況 (管理者権限のアプリが
/// 前面の時など) を再現し、最前面表示・やり直し・見張りの補いを確かめる。
pub fn simulate_failure() -> bool {
    #[cfg(debug_assertions)]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("LAUNCHPANEL_TEST_FG_FAIL").is_ok_and(|v| v == "1"))
    }
    #[cfg(not(debug_assertions))]
    false
}

unsafe fn try_set(hwnd: HWND) -> bool {
    // SAFETY: 呼び出し側の保証どおり
    unsafe { SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd }
}
