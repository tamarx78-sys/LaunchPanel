//! OLE のドロップ先。ファイル・フォルダーに加え、ブラウザーからのリンク (URL) も受け取る。
//!
//! WM_DROPFILES (DragAcceptFiles) はファイルしか受け取れないので、URL を受けたいウィンドウは
//! これを登録する。受け取った内容は `WM_APP_DROP` でウィンドウへ通知する。
//! ドロップの効果はリンクかコピーだけを返し、Explorer にファイルを移動させることはない。

use std::cell::Cell;

use windows::Win32::Foundation::{HWND, LPARAM, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{Ref, Result, implement, w};

/// ドロップ関連の通知。wParam: 0 = 受け取れる内容が上に来た、1 = 離れた、2 = ドロップされた
/// (lParam は `Box<String>` のポインタで、受け取り側が [`take_dropped`] で取り出す)。
pub const WM_APP_DROP: u32 = WM_APP + 20;
pub const DROP_ENTER: usize = 0;
pub const DROP_LEAVE: usize = 1;
pub const DROP_DONE: usize = 2;

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
    accepted: Cell<bool>,
}

/// `hwnd` をドロップ先として登録する。UI スレッドは OleInitialize 済みであること。
pub fn register(hwnd: HWND) -> bool {
    let target: IDropTarget = DropTarget { hwnd, accepted: Cell::new(false) }.into();
    // SAFETY: 自分のウィンドウへの登録 (OLE が参照を保持する)
    unsafe { RegisterDragDrop(hwnd, &target).is_ok() }
}

pub fn revoke(hwnd: HWND) {
    // SAFETY: register と対
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

/// `WM_APP_DROP` (DROP_DONE) の lParam から、ドロップされたパスまたは URL を取り出す。
///
/// # Safety
/// `lparam` は DROP_DONE の通知で受け取った値をそのまま 1 回だけ渡すこと。
pub unsafe fn take_dropped(lparam: LPARAM) -> String {
    // SAFETY: Drop で Box::into_raw したポインタ
    *unsafe { Box::from_raw(lparam.0 as *mut String) }
}

impl DropTarget {
    fn notify(&self, kind: usize, payload: Option<String>) {
        let lparam = payload.map_or(0, |s| Box::into_raw(Box::new(s)) as isize);
        // SAFETY: 自分のウィンドウへの通知
        unsafe {
            if PostMessageW(Some(self.hwnd), WM_APP_DROP, WPARAM(kind), LPARAM(lparam)).is_err() && lparam != 0 {
                drop(Box::from_raw(lparam as *mut String));
            }
        }
    }

    /// 移動は許さず、リンクかコピーを選ぶ。
    fn effect(&self, allowed: DROPEFFECT) -> DROPEFFECT {
        if !self.accepted.get() {
            DROPEFFECT_NONE
        } else if allowed.0 & DROPEFFECT_LINK.0 != 0 {
            DROPEFFECT_LINK
        } else if allowed.0 & DROPEFFECT_COPY.0 != 0 {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.accepted.set(data.ok().is_ok_and(|d| extract(d).is_some()));
        // SAFETY: OLE が渡す有効なポインタ
        unsafe { *effect = self.effect(*effect) };
        if self.accepted.get() {
            self.notify(DROP_ENTER, None);
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // SAFETY: OLE が渡す有効なポインタ
        unsafe { *effect = self.effect(*effect) };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.accepted.set(false);
        self.notify(DROP_LEAVE, None);
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let dropped = data.ok().ok().and_then(extract);
        self.accepted.set(dropped.is_some());
        // SAFETY: OLE が渡す有効なポインタ
        unsafe { *effect = self.effect(*effect) };
        self.notify(DROP_LEAVE, None);
        if dropped.is_some() {
            self.notify(DROP_DONE, dropped);
        }
        self.accepted.set(false);
        Ok(())
    }
}

/// ドロップされた内容から、最初のファイル・フォルダーのパス、または URL を取り出す。
fn extract(data: &IDataObject) -> Option<String> {
    if let Some(path) = hglobal(data, CF_HDROP.0, |h| {
        // SAFETY: CF_HDROP の HGLOBAL は HDROP
        unsafe {
            let hdrop = HDROP(h);
            let len = DragQueryFileW(hdrop, 0, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, 0, Some(&mut buf)) as usize;
            (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
        }
    }) {
        return Some(path);
    }
    // SAFETY: 書式名の登録 (既にあれば同じ値が返る)
    let url_format = unsafe { RegisterClipboardFormatW(w!("UniformResourceLocatorW")) } as u16;
    for format in [url_format, CF_UNICODETEXT.0] {
        let text = hglobal(data, format, |h| {
            // SAFETY: 文字列の HGLOBAL を読む
            unsafe {
                let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h)) as *const u16;
                if p.is_null() {
                    return None;
                }
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h));
                Some(s)
            }
        });
        if let Some(url) = text.map(|t| t.trim().to_owned()).filter(|t| is_url(t)) {
            return Some(url);
        }
    }
    None
}

fn is_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    !s.contains(char::is_whitespace) && (lower.starts_with("https://") || lower.starts_with("http://"))
}

/// 指定した書式の HGLOBAL を `read` に渡して読み、媒体を解放する。
fn hglobal<T>(data: &IDataObject, format: u16, read: impl FnOnce(*mut core::ffi::c_void) -> Option<T>) -> Option<T> {
    let fmt = FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: GetData で得た媒体は必ず解放する
    unsafe {
        let mut medium = data.GetData(&fmt).ok()?;
        let result = read(medium.u.hGlobal.0);
        ReleaseStgMedium(&mut medium);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::is_url;

    #[test]
    fn urls() {
        assert!(is_url("https://chatgpt.com/"));
        assert!(is_url("HTTP://example.com"));
        assert!(!is_url("ftp://example.com"));
        assert!(!is_url("https://a b"));
        assert!(!is_url("C:\\file.txt"));
    }
}
