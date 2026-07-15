use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::ptr::{null_mut, write_bytes};

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, SearchPathW};
use windows::Win32::UI::Shell::{
    ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW, SHFILEINFOW, SHGFI_ICON, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON};
use windows::core::{PCWSTR, PWSTR};

pub const ICON_SIZE: u32 = 32;

pub struct IconImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Creates a thumbnail for a local image, or resolves the Windows Shell icon
/// for an executable, shortcut, associated file, folder, or HTTP(S) URL.
/// Failed lookups use the UI fallback.
pub fn load(path: &str) -> Option<IconImage> {
    let resolved_path = match url_scheme(path) {
        Some(scheme) => resolve_url_handler(scheme)?,
        None => {
            if let Some(thumbnail) = load_image_thumbnail(path) {
                return Some(thumbnail);
            }
            resolve_path(path)?
        }
    };
    let wide_path: Vec<u16> = resolved_path.encode_utf16().chain(Some(0)).collect();
    let mut file_info = SHFILEINFOW::default();

    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide_path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut file_info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON,
        )
    };

    if result == 0 || file_info.hIcon.0.is_null() {
        return None;
    }

    let image = icon_to_rgba(file_info.hIcon);
    unsafe {
        let _ = DestroyIcon(file_info.hIcon);
    }
    image
}

fn load_image_thumbnail(path: &str) -> Option<IconImage> {
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumbnail = image.thumbnail(ICON_SIZE, ICON_SIZE).into_rgba8();
    let thumbnail_width = thumbnail.width() as usize;
    let thumbnail_height = thumbnail.height() as usize;
    let icon_size = ICON_SIZE as usize;
    let x_offset = (icon_size - thumbnail_width) / 2;
    let y_offset = (icon_size - thumbnail_height) / 2;
    let mut rgba = vec![0_u8; icon_size * icon_size * 4];

    for (y, row) in thumbnail
        .as_raw()
        .chunks_exact(thumbnail_width * 4)
        .enumerate()
    {
        let destination_start = ((y + y_offset) * icon_size + x_offset) * 4;
        rgba[destination_start..destination_start + row.len()].copy_from_slice(row);
    }

    // The egui texture is uploaded as premultiplied RGBA, matching DrawIconEx output.
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        pixel[0] = ((u16::from(pixel[0]) * alpha + 127) / 255) as u8;
        pixel[1] = ((u16::from(pixel[1]) * alpha + 127) / 255) as u8;
        pixel[2] = ((u16::from(pixel[2]) * alpha + 127) / 255) as u8;
    }

    Some(IconImage {
        width: icon_size,
        height: icon_size,
        rgba,
    })
}

fn url_scheme(path: &str) -> Option<&'static str> {
    if path
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    {
        Some("http")
    } else if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        Some("https")
    } else {
        None
    }
}

fn resolve_url_handler(scheme: &str) -> Option<String> {
    let wide_scheme: Vec<u16> = scheme.encode_utf16().chain(Some(0)).collect();
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;

    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(wide_scheme.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    };

    if result.is_err() || length == 0 || length as usize > buffer.len() {
        return None;
    }

    let string_length = buffer[..length as usize]
        .iter()
        .position(|&character| character == 0)
        .unwrap_or(length as usize);
    String::from_utf16(&buffer[..string_length]).ok()
}

fn resolve_path(path: &str) -> Option<String> {
    if Path::new(path).exists() {
        return Some(path.to_owned());
    }

    let wide_path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let exe_extension: Vec<u16> = ".exe".encode_utf16().chain(Some(0)).collect();
    let extension = if Path::new(path).extension().is_none() {
        PCWSTR(exe_extension.as_ptr())
    } else {
        PCWSTR::null()
    };
    let mut buffer = vec![0_u16; 32_768];

    let length = unsafe {
        SearchPathW(
            PCWSTR::null(),
            PCWSTR(wide_path.as_ptr()),
            extension,
            Some(&mut buffer),
            None,
        )
    } as usize;

    if length == 0 || length >= buffer.len() {
        return None;
    }

    String::from_utf16(&buffer[..length]).ok()
}

fn icon_to_rgba(icon: HICON) -> Option<IconImage> {
    let size = ICON_SIZE as i32;
    let byte_len = ICON_SIZE as usize * ICON_SIZE as usize * 4;
    let mut bitmap_info = BITMAPINFO::default();
    bitmap_info.bmiHeader.biSize =
        size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
    bitmap_info.bmiHeader.biWidth = size;
    // Negative height creates a top-down bitmap, matching egui's row order.
    bitmap_info.bmiHeader.biHeight = -size;
    bitmap_info.bmiHeader.biPlanes = 1;
    bitmap_info.bmiHeader.biBitCount = 32;
    bitmap_info.bmiHeader.biCompression = BI_RGB.0;

    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.0.is_null() {
        return None;
    }

    let mut bits: *mut c_void = null_mut();
    let bitmap = match unsafe {
        CreateDIBSection(Some(dc), &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)
    } {
        Ok(bitmap) if !bits.is_null() => bitmap,
        _ => {
            unsafe {
                let _ = DeleteDC(dc);
            }
            return None;
        }
    };

    let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
    unsafe {
        write_bytes(bits.cast::<u8>(), 0, byte_len);
    }

    let drawn = unsafe { DrawIconEx(dc, 0, 0, icon, size, size, 0, None, DI_NORMAL) }.is_ok();
    let bgra = if drawn {
        Some(unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), byte_len) }.to_vec())
    } else {
        None
    };

    unsafe {
        if !previous.0.is_null() {
            let _ = SelectObject(dc, previous);
        }
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
    }

    let mut rgba = bgra?;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Some(IconImage {
        width: ICON_SIZE as usize,
        height: ICON_SIZE as usize,
        rgba,
    })
}
