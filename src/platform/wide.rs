//! UTF-16 文字列の受け渡しヘルパー。

/// NUL 終端の UTF-16 ポインタを String へ変換する。null なら空文字。
///
/// # Safety
/// `ptr` は null か、NUL 終端された有効な UTF-16 文字列を指すこと。
pub unsafe fn from_ptr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: 呼び出し側が NUL 終端を保証する
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

/// NUL 終端付きの UTF-16 へ変換する。
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
