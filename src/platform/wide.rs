//! UTF-16 文字列と環境変数のヘルパー。

use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::core::PCWSTR;

/// NUL 終端付きの UTF-16 へ変換する。
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `%USERPROFILE%` などの環境変数を展開する。展開できなければそのまま返す。
pub fn expand_env(path: &str) -> String {
    if !path.contains('%') {
        return path.to_owned();
    }
    let src = to_wide(path);
    let mut buf = vec![0u16; 32768];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return path.to_owned();
    }
    String::from_utf16_lossy(&buf[..n - 1])
}
