//! アイテムのアイコン/サムネイル取得。
//!
//! `IShellItemImageFactory` を使い、画像ファイルはサムネイル、実行ファイル・ショートカット・
//! 関連付け済みファイル・フォルダーはシェルのアイコンを取得する。URL などのプロトコルは
//! 関連付けられたハンドラー (既定ブラウザーなど) のアイコンで代用する。
//! パスが直接存在しない実行可能名は SearchPath と App Paths レジストリで解決を試みる。
//!
//! COM を使うため、呼び出しスレッドは COM 初期化済み (STA 推奨) であること。

use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_SUCCESS, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::{
    GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SearchPathW,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::Shell::{
    ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, AssocQueryStringW, IShellItemImageFactory,
    SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
};
use windows::core::{PCWSTR, PWSTR, w};

use crate::platform::wide::to_wide;

#[derive(Clone)]
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
}

pub fn load(path: &str, size: i32) -> Option<Pixels> {
    let target = resolve(path)?;
    let wide = to_wide(&target);
    // SAFETY: COM 呼び出し。HBITMAP は extract_pixels 後に必ず解放する
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let bitmap = factory
            .GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT)
            .ok()?;
        let pixels = extract_pixels(bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        pixels
    }
}

/// アイコン取得の対象となるシェル解析可能な名前へ解決する。
fn resolve(path: &str) -> Option<String> {
    let expanded = expand_env(path.trim());
    if expanded.is_empty() {
        return None;
    }
    if let Some(scheme) = protocol_scheme(&expanded) {
        return assoc_executable(scheme);
    }
    if exists(&expanded) {
        return Some(expanded);
    }
    if let Some(found) = search_path(&expanded).or_else(|| app_path(&expanded)) {
        return Some(found);
    }
    // shell: や ::{GUID} などはシェルが直接解析できる
    Some(expanded)
}

/// "https://..." や "mailto:" のようなプロトコルのスキーム部分。ドライブ文字 "C:" は除く。
fn protocol_scheme(path: &str) -> Option<&str> {
    let colon = path.find(':')?;
    let scheme = &path[..colon];
    let valid = colon > 1
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    (valid && !scheme.eq_ignore_ascii_case("shell")).then_some(scheme)
}

fn assoc_executable(scheme: &str) -> Option<String> {
    let scheme = to_wide(scheme);
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: バッファ長を渡している
    unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(scheme.as_ptr()),
            w!("open"),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
    }
    let s = String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]);
    exists(&s).then_some(s)
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
        return path.to_owned();
    }
    String::from_utf16_lossy(&buf[..n - 1])
}

fn exists(path: &str) -> bool {
    let wide = to_wide(path);
    // SAFETY: NUL 終端文字列
    unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) != INVALID_FILE_ATTRIBUTES }
}

fn search_path(name: &str) -> Option<String> {
    let wide = to_wide(name);
    let mut buf = [0u16; 1024];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe {
        SearchPathW(
            None,
            PCWSTR(wide.as_ptr()),
            w!(".exe"),
            Some(&mut buf),
            None,
        )
    } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// HKCU/HKLM の App Paths に登録された実行可能名を解決する。
fn app_path(name: &str) -> Option<String> {
    let file = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_owned()
    } else {
        format!("{name}.exe")
    };
    let subkey = to_wide(&format!(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\{file}"
    ));
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| read_default(root, &subkey))
}

fn read_default(root: HKEY, subkey: &[u16]) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: バッファ長をバイト数で渡している
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            None,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1);
    let s = String::from_utf16_lossy(&buf[..len])
        .trim_matches('"')
        .to_owned();
    exists(&s).then_some(s)
}

/// HBITMAP から 32bpp トップダウンの画素を取り出し、乗算済みアルファに正規化する。
unsafe fn extract_pixels(bitmap: HBITMAP) -> Option<Pixels> {
    // SAFETY: GDI 呼び出し。DC は関数内で生成・解放する
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(bitmap.0),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut c_void),
        ) == 0
        {
            return None;
        }
        let (width, height) = (bm.bmWidth, bm.bmHeight.abs());
        if width <= 0 || height <= 0 {
            return None;
        }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // 負値でトップダウン
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut data = vec![0u8; (width * height * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(data.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        if lines == 0 {
            return None;
        }
        normalize_alpha(&mut data);
        Some(Pixels {
            width,
            height,
            data,
        })
    }
}

/// サムネイル (JPEG など) はアルファが全て 0 で返ることがあるので不透明にする。
/// 乗算済みでない (色 > アルファの画素がある) 場合は乗算する。
fn normalize_alpha(data: &mut [u8]) {
    if data.chunks_exact(4).all(|p| p[3] == 0) {
        data.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        return;
    }
    let premultiplied = data
        .chunks_exact(4)
        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    if !premultiplied {
        for p in data.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_detection() {
        assert_eq!(protocol_scheme("https://example.com"), Some("https"));
        assert_eq!(protocol_scheme("ms-settings:display"), Some("ms-settings"));
        assert_eq!(protocol_scheme(r"C:\Windows"), None);
        assert_eq!(protocol_scheme("shell:Downloads"), None);
        assert_eq!(protocol_scheme("notepad"), None);
    }

    #[test]
    fn transparent_thumbnail_becomes_opaque() {
        let mut d = vec![10, 20, 30, 0, 40, 50, 60, 0];
        normalize_alpha(&mut d);
        assert_eq!(d, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn straight_alpha_is_premultiplied() {
        let mut d = vec![255, 255, 255, 128];
        normalize_alpha(&mut d);
        assert_eq!(d, vec![128, 128, 128, 128]);
    }

    /// 実機のシェルに依存する (既定のブラウザーなど)。手動実行: cargo test -- --ignored
    #[test]
    #[ignore]
    fn resolves_executable_name_and_url() {
        // SAFETY: テストスレッドでの COM 初期化
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        assert!(resolve("notepad").is_some_and(|p| p.to_lowercase().ends_with("notepad.exe")));
        assert!(load("notepad", 32).is_some());
        assert!(load(r"C:\Windows", 32).is_some());
        assert!(load("https://example.com", 32).is_some());
    }
}
