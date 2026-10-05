//! ダイアログ用のコントロールの描画 (スイッチ・スライダー・ボタンなど)。
//! 入力の扱いは各ダイアログが持ち、ここは見た目だけを共通化する。

use crate::render::{Color, Gfx, Rect, TextStyle};

pub const BG: Color = Color::rgba(0x20, 0x21, 0x25, 0xFF);
pub const PANEL: Color = Color::rgba(0x2A, 0x2B, 0x31, 0xFF);
pub const CONTROL: Color = Color::rgba(0x37, 0x39, 0x40, 0xFF);
pub const CONTROL_HOVER: Color = Color::rgba(0x43, 0x46, 0x4F, 0xFF);
pub const BORDER: Color = Color::rgba(0x4A, 0x4D, 0x57, 0xFF);
pub const TEXT: Color = Color::rgba(0xE8, 0xE8, 0xEC, 0xFF);
pub const MUTED: Color = Color::rgba(0xA0, 0xA3, 0xAB, 0xFF);
pub const DISABLED: Color = Color::rgba(0x6A, 0x6D, 0x75, 0xFF);
pub const ACCENT: Color = Color::rgba(0x4C, 0xA0, 0xFF, 0xFF);
pub const ACCENT_HOVER: Color = Color::rgba(0x6C, 0xB4, 0xFF, 0xFF);
pub const ERROR: Color = Color::rgba(0xFF, 0x7B, 0x6B, 0xFF);
pub const WARNING: Color = Color::rgba(0xF0, 0xC0, 0x50, 0xFF);

/// 操作対象の見た目の状態。
#[derive(Clone, Copy, Default)]
pub struct State {
    pub hover: bool,
    pub active: bool,
    pub disabled: bool,
}

pub fn button(gfx: &Gfx, r: Rect, label: &str, accent: bool, st: State) {
    let fill = match (accent, st.disabled, st.hover || st.active) {
        (_, true, _) => CONTROL,
        (true, _, true) => ACCENT_HOVER,
        (true, _, false) => ACCENT,
        (false, _, true) => CONTROL_HOVER,
        (false, _, false) => CONTROL,
    };
    gfx.fill_round(r, 6.0, fill);
    let text = if st.disabled {
        DISABLED
    } else if accent {
        Color::rgba(0x10, 0x14, 0x1A, 0xFF)
    } else {
        TEXT
    };
    gfx.text(label, r.inset(8.0, 0.0), text, TextStyle::Value);
}

/// オン/オフのスイッチ。オンは右へ寄った丸、アクセント色の地 (色と位置の両方で示す)。
pub fn switch(gfx: &Gfx, r: Rect, on: bool, st: State) {
    let track = if st.disabled {
        CONTROL
    } else if on {
        if st.hover { ACCENT_HOVER } else { ACCENT }
    } else if st.hover {
        CONTROL_HOVER
    } else {
        CONTROL
    };
    gfx.fill_round(r, r.h / 2.0, track);
    if !on {
        gfx.stroke_round(r, r.h / 2.0, BORDER, 1.0);
    }
    let d = r.h - 8.0;
    let x = if on { r.x + r.w - 4.0 - d } else { r.x + 4.0 };
    let knob = if on { Color::rgba(0x10, 0x14, 0x1A, 0xFF) } else { MUTED };
    gfx.fill_round(Rect::new(x, r.y + 4.0, d, d), d / 2.0, knob);
}

/// スライダー。`frac` は 0.0～1.0。
pub fn slider(gfx: &Gfx, r: Rect, frac: f32, st: State) {
    let frac = frac.clamp(0.0, 1.0);
    let track = Rect::new(r.x, r.y + r.h / 2.0 - 2.0, r.w, 4.0);
    gfx.fill_round(track, 2.0, CONTROL_HOVER);
    let filled = Rect::new(track.x, track.y, track.w * frac, track.h);
    gfx.fill_round(filled, 2.0, if st.disabled { DISABLED } else { ACCENT });
    let d = if st.hover || st.active { 18.0 } else { 16.0 };
    let cx = r.x + r.w * frac;
    let knob = Rect::new(cx - d / 2.0, r.y + r.h / 2.0 - d / 2.0, d, d);
    gfx.fill_round(knob, d / 2.0, if st.disabled { DISABLED } else { TEXT });
    gfx.fill_round(knob.inset(4.0, 4.0), (d - 8.0) / 2.0, if st.disabled { CONTROL } else { ACCENT });
}

/// スライダー上の位置 `x` から 0.0～1.0 を求める。
pub fn slider_frac(r: Rect, x: f32) -> f32 {
    ((x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0)
}

/// ドラッグで値を変える欄。左右の矢印でドラッグ方向を示す。
pub fn scrubber(gfx: &Gfx, r: Rect, value: &str, st: State) {
    gfx.fill_round(r, 6.0, if st.hover || st.active { CONTROL_HOVER } else { CONTROL });
    if st.active {
        gfx.stroke_round(r, 6.0, ACCENT, 1.5);
    }
    gfx.text("\u{E76B}", Rect::new(r.x + 4.0, r.y, 20.0, r.h), MUTED, TextStyle::Glyph);
    gfx.text("\u{E76C}", Rect::new(r.x + r.w - 24.0, r.y, 20.0, r.h), MUTED, TextStyle::Glyph);
    gfx.text(value, r.inset(24.0, 0.0), TEXT, TextStyle::Value);
}

/// 押し込み状態を持つトグルボタン (修飾キーなど)。
pub fn toggle(gfx: &Gfx, r: Rect, label: &str, on: bool, st: State) {
    let fill = match (st.disabled, on, st.hover) {
        (true, _, _) => PANEL,
        (_, true, true) => ACCENT_HOVER,
        (_, true, false) => ACCENT,
        (_, false, true) => CONTROL_HOVER,
        (_, false, false) => CONTROL,
    };
    gfx.fill_round(r, 6.0, fill);
    if st.disabled {
        gfx.stroke_round(r, 6.0, CONTROL, 1.0);
    }
    let text = if st.disabled {
        DISABLED
    } else if on {
        Color::rgba(0x10, 0x14, 0x1A, 0xFF)
    } else {
        TEXT
    };
    gfx.text(label, r, text, TextStyle::Value);
}

/// 一覧から選ぶボタン (右端に下向きの矢印)。
pub fn dropdown(gfx: &Gfx, r: Rect, value: &str, st: State) {
    gfx.fill_round(r, 6.0, if st.hover || st.active { CONTROL_HOVER } else { CONTROL });
    gfx.text(value, Rect::new(r.x, r.y, r.w - 20.0, r.h), TEXT, TextStyle::Value);
    gfx.text("\u{E70D}", Rect::new(r.x + r.w - 26.0, r.y, 20.0, r.h), MUTED, TextStyle::Glyph);
}

/// 色見本。押すと色の選択パネルが開く。
pub fn swatch(gfx: &Gfx, r: Rect, color: Color, st: State) {
    gfx.fill_round(r, 6.0, if st.hover || st.active { ACCENT } else { BORDER });
    gfx.fill_round(r.inset(2.0, 2.0), 4.5, color);
}

/// アイテムボタン (アイコンと名前)。メインウィンドウと設定画面のプレビューで共通。
pub fn item_button(
    gfx: &Gfx,
    r: Rect,
    name: &str,
    icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    s: &crate::config::ItemButtonSettings,
    hovered: bool,
    opacity: f32,
) {
    const ICON: f32 = 20.0;
    let bg = s.background_color;
    gfx.fill_round(r, 5.0, Color::rgba(bg.0, bg.1, bg.2, 255).with_alpha(s.background_alpha() * opacity));
    if let Some(b) = icon {
        gfx.bitmap(b, Rect::new(r.x + 8.0, r.y + (r.h - ICON) / 2.0, ICON, ICON), opacity);
    }
    let tc = s.text_color;
    let style = if s.bold_text { TextStyle::ItemBold } else { TextStyle::Item };
    gfx.text(
        name,
        Rect::new(r.x + 36.0, r.y, (r.w - 44.0).max(0.0), r.h),
        Color::rgba(tc.0, tc.1, tc.2, 255).with_alpha(opacity),
        style,
    );
    if hovered {
        // 明るい輪郭と薄い白の重ねで、背景の明暗によらず識別できるようにする
        gfx.fill_round(r, 5.0, Color::rgba(0xFF, 0xFF, 0xFF, 0x24).with_alpha(0x24 as f32 / 255.0 * opacity));
        gfx.stroke_round(r, 5.0, ACCENT_HOVER.with_alpha(opacity), 2.0);
    }
}
