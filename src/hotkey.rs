use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, RegisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

pub fn key_to_vk(key: &str) -> Option<u32> {
    match key {
        "A" => Some(b'A' as u32),
        "B" => Some(b'B' as u32),
        "C" => Some(b'C' as u32),
        "D" => Some(b'D' as u32),
        "E" => Some(b'E' as u32),
        "F" => Some(b'F' as u32),
        "G" => Some(b'G' as u32),
        "H" => Some(b'H' as u32),
        "I" => Some(b'I' as u32),
        "J" => Some(b'J' as u32),
        "K" => Some(b'K' as u32),
        "L" => Some(b'L' as u32),
        "M" => Some(b'M' as u32),
        "N" => Some(b'N' as u32),
        "O" => Some(b'O' as u32),
        "P" => Some(b'P' as u32),
        "Q" => Some(b'Q' as u32),
        "R" => Some(b'R' as u32),
        "S" => Some(b'S' as u32),
        "T" => Some(b'T' as u32),
        "U" => Some(b'U' as u32),
        "V" => Some(b'V' as u32),
        "W" => Some(b'W' as u32),
        "X" => Some(b'X' as u32),
        "Y" => Some(b'Y' as u32),
        "Z" => Some(b'Z' as u32),
        _ => None,
    }
}
pub fn modifiers(ctrl: bool, alt: bool, shift: bool, win: bool) -> HOT_KEY_MODIFIERS {
    let mut m = HOT_KEY_MODIFIERS(0);

    if ctrl {
        m |= MOD_CONTROL;
    }

    if alt {
        m |= MOD_ALT;
    }

    if shift {
        m |= MOD_SHIFT;
    }

    if win {
        m |= MOD_WIN;
    }

    m
}
pub fn register(id: i32, ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> bool {
    let Some(vk) = key_to_vk(key) else {
        return false;
    };
    let modifiers = modifiers(ctrl, alt, shift, win);

    unsafe { RegisterHotKey(None, id, modifiers, vk).is_ok() }
}
pub fn run<F>(
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: &str,
    mut callback: F,
)
where
    F: FnMut(),
{
    const HOTKEY_ID: i32 = 1;

    if !register(HOTKEY_ID, ctrl, alt, shift, win, key) {
        eprintln!("ホットキー登録に失敗しました");
        return;
    }

    let mut msg = MSG::default();

    loop {
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };

        if result.0 <= 0 {
            break;
        }

        if msg.message == WM_HOTKEY {
            callback();
        }
    }
}
