//! ダイアログ用のコントロールの描画 (スイッチ・スライダー・ボタンなど)。
//! 入力の扱いは各ダイアログが持ち、ここは見た目だけを共通化する。

use crate::render::{Color, Gfx, Rect, TextStyle};
use crate::skin::Style;

/// いまのスキンの配色 (`ui::pal().text` のように使う)。
pub use crate::skin::palette as pal;

/// 角丸の半径をスキンに合わせる (角張るスキンでは 0)。
pub fn rad(r: f32) -> f32 {
    r * pal().radius
}

/// 操作対象の見た目の状態。
#[derive(Clone, Copy, Default)]
pub struct State {
    pub hover: bool,
    pub active: bool,
    pub disabled: bool,
}

pub fn button(gfx: &Gfx, r: Rect, label: &str, accent: bool, st: State) {
    let p = pal();
    let fill = match (accent, st.disabled, st.hover || st.active) {
        (_, true, _) => p.control,
        (true, _, true) => p.accent_hover,
        (true, _, false) => p.accent,
        (false, _, true) => p.control_hover,
        (false, _, false) => p.control,
    };
    gfx.fill_round(r, rad(6.0), fill);
    if p.style == Style::Terminal && !accent {
        // 端末風は塗りの差が小さいので、枠線で部品の範囲を示す
        let edge = if st.hover && !st.disabled {
            p.accent
        } else {
            p.border
        };
        gfx.stroke_round(r, rad(6.0), edge, 1.0);
    }
    let text = if st.disabled {
        p.disabled
    } else if accent {
        p.on_accent
    } else {
        p.text
    };
    gfx.text(label, r.inset(8.0, 0.0), text, TextStyle::Value);
}

/// オン/オフのスイッチ。オンは右へ寄った丸、アクセント色の地 (色と位置の両方で示す)。
pub fn switch(gfx: &Gfx, r: Rect, on: bool, st: State) {
    let p = pal();
    let track = if st.disabled {
        p.control
    } else if on {
        if st.hover { p.accent_hover } else { p.accent }
    } else if st.hover {
        p.control_hover
    } else {
        p.control
    };
    gfx.fill_round(r, rad(r.h / 2.0), track);
    if !on {
        gfx.stroke_round(r, rad(r.h / 2.0), p.border, 1.0);
    }
    let d = r.h - 8.0;
    let x = if on { r.x + r.w - 4.0 - d } else { r.x + 4.0 };
    let knob = if on { p.on_accent } else { p.muted };
    gfx.fill_round(Rect::new(x, r.y + 4.0, d, d), rad(d / 2.0), knob);
}

/// スライダー。`frac` は 0.0～1.0。
pub fn slider(gfx: &Gfx, r: Rect, frac: f32, st: State) {
    let p = pal();
    let frac = frac.clamp(0.0, 1.0);
    let track = Rect::new(r.x, r.y + r.h / 2.0 - 2.0, r.w, 4.0);
    gfx.fill_round(track, rad(2.0), p.control_hover);
    let filled = Rect::new(track.x, track.y, track.w * frac, track.h);
    gfx.fill_round(
        filled,
        rad(2.0),
        if st.disabled { p.disabled } else { p.accent },
    );
    let d = if st.hover || st.active { 18.0 } else { 16.0 };
    let cx = r.x + r.w * frac;
    let knob = Rect::new(cx - d / 2.0, r.y + r.h / 2.0 - d / 2.0, d, d);
    gfx.fill_round(
        knob,
        rad(d / 2.0),
        if st.disabled { p.disabled } else { p.text },
    );
    gfx.fill_round(
        knob.inset(4.0, 4.0),
        rad((d - 8.0) / 2.0),
        if st.disabled { p.control } else { p.accent },
    );
}

/// スライダー上の位置 `x` から 0.0～1.0 を求める。
pub fn slider_frac(r: Rect, x: f32) -> f32 {
    ((x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0)
}

/// ドラッグで値を変える欄。左右の矢印でドラッグ方向を示す。
pub fn scrubber(gfx: &Gfx, r: Rect, value: &str, st: State) {
    let p = pal();
    gfx.fill_round(
        r,
        rad(6.0),
        if st.hover || st.active {
            p.control_hover
        } else {
            p.control
        },
    );
    if st.active {
        gfx.stroke_round(r, rad(6.0), p.accent, 1.5);
    }
    gfx.text(
        "\u{E76B}",
        Rect::new(r.x + 4.0, r.y, 20.0, r.h),
        p.muted,
        TextStyle::Glyph,
    );
    gfx.text(
        "\u{E76C}",
        Rect::new(r.x + r.w - 24.0, r.y, 20.0, r.h),
        p.muted,
        TextStyle::Glyph,
    );
    gfx.text(value, r.inset(24.0, 0.0), p.text, TextStyle::Value);
}

/// 押し込み状態を持つトグルボタン (修飾キーなど)。
pub fn toggle(gfx: &Gfx, r: Rect, label: &str, on: bool, st: State) {
    let p = pal();
    let fill = match (st.disabled, on, st.hover) {
        (true, _, _) => p.panel,
        (_, true, true) => p.accent_hover,
        (_, true, false) => p.accent,
        (_, false, true) => p.control_hover,
        (_, false, false) => p.control,
    };
    gfx.fill_round(r, rad(6.0), fill);
    if st.disabled {
        gfx.stroke_round(r, rad(6.0), p.control, 1.0);
    }
    let text = if st.disabled {
        p.disabled
    } else if on {
        p.on_accent
    } else {
        p.text
    };
    gfx.text(label, r, text, TextStyle::Value);
}

/// 一覧から選ぶボタン (右端に下向きの矢印)。
pub fn dropdown(gfx: &Gfx, r: Rect, value: &str, st: State) {
    let p = pal();
    gfx.fill_round(
        r,
        rad(6.0),
        if st.hover || st.active {
            p.control_hover
        } else {
            p.control
        },
    );
    gfx.text(
        value,
        Rect::new(r.x, r.y, r.w - 20.0, r.h),
        p.text,
        TextStyle::Value,
    );
    gfx.text(
        "\u{E70D}",
        Rect::new(r.x + r.w - 26.0, r.y, 20.0, r.h),
        p.muted,
        TextStyle::Glyph,
    );
}

/// 色見本。押すと色の選択パネルが開く。
pub fn swatch(gfx: &Gfx, r: Rect, color: Color, st: State) {
    let p = pal();
    gfx.fill_round(
        r,
        rad(6.0),
        if st.hover || st.active {
            p.accent
        } else {
            p.border
        },
    );
    gfx.fill_round(r.inset(2.0, 2.0), rad(4.5), color);
}

/// アイテムボタン (アイコンと名前)。メインウィンドウと設定画面のプレビューで共通。
/// `color` はボタンの色番号 (0 = 標準、1～5 = グループの色)。描き方はスキンによる。
#[allow(clippy::too_many_arguments)]
pub fn item_button(
    gfx: &Gfx,
    r: Rect,
    name: &str,
    icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    s: &crate::config::ItemButtonSettings,
    color: usize,
    hovered: bool,
    opacity: f32,
) {
    match pal().style {
        Style::Modern => item_button_modern(gfx, r, name, icon, s, color, hovered, opacity),
        Style::Terminal => item_button_terminal(gfx, r, name, icon, s, color, hovered, opacity),
    }
}

#[allow(clippy::too_many_arguments)]
fn item_button_modern(
    gfx: &Gfx,
    r: Rect,
    name: &str,
    icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    s: &crate::config::ItemButtonSettings,
    color: usize,
    hovered: bool,
    opacity: f32,
) {
    let p = pal();
    const ICON: f32 = 20.0;
    let bg = s.color(color);
    gfx.fill_round(
        r,
        5.0,
        Color::rgba(bg.0, bg.1, bg.2, 255).with_alpha(s.background_alpha() * opacity),
    );
    if let Some(b) = icon {
        gfx.bitmap(
            b,
            Rect::new(r.x + 8.0, r.y + (r.h - ICON) / 2.0, ICON, ICON),
            opacity,
        );
    }
    let tc = s.text_color;
    let style = if s.bold_text {
        TextStyle::ItemBold
    } else {
        TextStyle::Item
    };
    gfx.text(
        name,
        Rect::new(r.x + 36.0, r.y, (r.w - 44.0).max(0.0), r.h),
        Color::rgba(tc.0, tc.1, tc.2, 255).with_alpha(opacity),
        style,
    );
    if hovered {
        // 明るい輪郭と薄い白の重ねで、背景の明暗によらず識別できるようにする
        gfx.fill_round(
            r,
            5.0,
            Color::rgba(0xFF, 0xFF, 0xFF, 0x24).with_alpha(0x24 as f32 / 255.0 * opacity),
        );
        gfx.stroke_round(r, 5.0, p.accent_hover.with_alpha(opacity), 2.0);
    }
}

/// 端末風のアイテムボタン。設定のボタンの色・透過率は使わず、スキンの配色で描く。
/// 枠線で囲み、強調 (ポインターを重ねた時) は端末の選択のように地と文字を反転する。
/// グループの色はボタン左端の帯で示す。
#[allow(clippy::too_many_arguments)]
fn item_button_terminal(
    gfx: &Gfx,
    r: Rect,
    name: &str,
    icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    s: &crate::config::ItemButtonSettings,
    color: usize,
    hovered: bool,
    opacity: f32,
) {
    const ICON: f32 = 20.0;
    let p = pal();
    let fade = |c: Color| c.with_alpha(c.3 * opacity);
    if hovered {
        gfx.fill_rect(r, fade(p.accent.with_alpha(0.92)));
    } else {
        gfx.fill_rect(r, fade(p.bg.with_alpha(0.55)));
        gfx.fill_rect(r, fade(p.accent.with_alpha(0.05)));
    }
    gfx.stroke_round(
        r,
        0.0,
        fade(if hovered { p.accent_hover } else { p.border }),
        1.0,
    );
    if (1..crate::config::BUTTON_COLORS).contains(&color) {
        let g = s.color(color);
        gfx.fill_rect(
            Rect::new(r.x + 1.0, r.y + 1.0, 4.0, (r.h - 2.0).max(0.0)),
            fade(Color::rgba(g.0, g.1, g.2, 255)),
        );
    }
    if let Some(b) = icon {
        let ir = Rect::new(r.x + 10.0, r.y + (r.h - ICON) / 2.0, ICON, ICON);
        if hovered {
            // 反転した地の上でも単色のアイコンが埋もれないよう、暗い台を敷く
            gfx.fill_rect(ir.inset(-2.0, -2.0), fade(p.on_accent));
        }
        gfx.bitmap(b, ir, opacity);
    }
    let style = if s.bold_text {
        TextStyle::ItemBold
    } else {
        TextStyle::Item
    };
    gfx.text(
        name,
        Rect::new(r.x + 38.0, r.y, (r.w - 46.0).max(0.0), r.h),
        fade(if hovered { p.on_accent } else { p.text }),
        style,
    );
}

/// メインウィンドウの上部・下部の小さな表示とボタンの地。
pub fn pill(gfx: &Gfx, r: Rect, hovered: bool) {
    let p = pal();
    gfx.fill_round(r, rad(6.0), if hovered { p.pill_hover } else { p.pill });
    if p.style == Style::Terminal {
        gfx.stroke_round(r, 0.0, if hovered { p.accent } else { p.border }, 1.0);
    }
}

/// メインウィンドウの区切り線 (端末風のみ)。上部の帯の下と下部の帯の上に引く。
pub fn chrome_lines(gfx: &Gfx, width: f32, top: f32, bottom: f32) {
    let p = pal();
    if p.style != Style::Terminal {
        return;
    }
    let line = p.border.with_alpha(0.9);
    for y in [top - 1.0, bottom] {
        gfx.fill_rect(Rect::new(8.0, y, (width - 16.0).max(0.0), 1.0), line);
    }
}

/// 走査線 (ブラウン管の横縞) を重ねる。スキンが指定しなければ何もしない。
pub fn scanlines(gfx: &Gfx, full: Rect) {
    let strength = pal().scanlines;
    if strength <= 0.0 {
        return;
    }
    let shade = Color(0.0, 0.0, 0.0, strength);
    let mut y = full.y;
    while y < full.y + full.h {
        gfx.fill_rect(Rect::new(full.x, y + 2.0, full.w, 1.0), shade);
        y += 3.0;
    }
}
