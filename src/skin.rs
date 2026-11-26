//! スキン (配色・字体・描き方の一式) の定義と、いま使っているスキン。
//!
//! 画面の各部 (メインウィンドウ・設定画面・編集画面) は色や字体を直接持たず、
//! [`palette`] から読む。スキンを切り替えると次の描画から全画面がその見た目になる
//! (字体とアイコンの作り直しは [`crate::render::Gfx`] が自動で行う)。
//!
//! # スキンの追加方法
//!
//! 1. [`Palette`] の定数を1つ定義する (既存の [`STANDARD`] / [`FALLOUT`] を写して色を変えるのが手早い)。
//! 2. [`SKINS`] に `Skin { id, name, palette }` を1行足す。
//!
//! これだけで設定画面の「スキン」の一覧に並び、設定ファイルにも `id` で保存される。
//! 描き方そのもの (角の形・ボタンの強調の仕方など) は [`Style`] で選ぶ。新しい描き方が
//! 必要なら [`Style`] に値を足し、`ui.rs` の描画関数に分岐を加える。

use std::cell::Cell;

use crate::platform::icon::Pixels;
use crate::render::Color;

/// 部品の描き方の系統。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// 角丸・塗りの濃淡で区切る標準の描き方
    Modern,
    /// 角張った枠線・反転表示の強調・走査線など、古い端末の画面のような描き方
    Terminal,
}

/// スキン1つ分の配色と描き方。
pub struct Palette {
    pub style: Style,
    /// 文字の字体の候補 (先頭から順に、インストールされている最初のもの)
    pub fonts: &'static [&'static str],
    /// 角丸の半径に掛ける倍率 (0 で角張る)
    pub radius: f32,

    // ── ダイアログ (設定画面・編集画面) ──
    pub bg: Color,
    pub panel: Color,
    pub control: Color,
    pub control_hover: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub disabled: Color,
    pub accent: Color,
    pub accent_hover: Color,
    /// アクセント色の上に置く文字・つまみの色
    pub on_accent: Color,
    pub error: Color,
    pub warning: Color,

    // ── メインウィンドウ ──
    /// 背景を描かない時 (単色) の地の色
    pub solid: Color,
    /// 設定の「暗さ」で重ねる色
    pub tint: Color,
    /// 背景 (画像・壁紙ぼかし・アクリル) の上に必ず重ねる色。スキンの雰囲気づけ用
    pub backdrop_overlay: Option<Color>,
    /// 上部・下部の小さな表示とボタンの地
    pub pill: Color,
    pub pill_hover: Color,
    pub overlay_text: Color,
    pub scrollbar: Color,
    /// アイテムのアイコンをこの色の単色にする (None ならそのまま)
    pub icon_tint: Option<Color>,
    /// 走査線の濃さ (0.0 でなし)
    pub scanlines: f32,
}

impl Palette {
    /// 設定の「ボタン」の背景色・テキスト色・透過率を使うか (使わないスキンは自前の配色で描く)。
    pub fn uses_button_colors(&self) -> bool {
        self.style == Style::Modern
    }
}

pub struct Skin {
    /// 設定ファイルに保存する名前 (変えないこと)
    pub id: &'static str,
    /// 設定画面での表示名
    pub name: &'static str,
    pub palette: Palette,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::rgba(r, g, b, 0xFF)
}

pub const STANDARD: Palette = Palette {
    style: Style::Modern,
    fonts: &["Segoe UI"],
    radius: 1.0,
    bg: rgb(0x20, 0x21, 0x25),
    panel: rgb(0x2A, 0x2B, 0x31),
    control: rgb(0x37, 0x39, 0x40),
    control_hover: rgb(0x43, 0x46, 0x4F),
    border: rgb(0x4A, 0x4D, 0x57),
    text: rgb(0xE8, 0xE8, 0xEC),
    muted: rgb(0xA0, 0xA3, 0xAB),
    disabled: rgb(0x6A, 0x6D, 0x75),
    accent: rgb(0x4C, 0xA0, 0xFF),
    accent_hover: rgb(0x6C, 0xB4, 0xFF),
    on_accent: rgb(0x10, 0x14, 0x1A),
    error: rgb(0xFF, 0x7B, 0x6B),
    warning: rgb(0xF0, 0xC0, 0x50),
    solid: rgb(0x22, 0x23, 0x28),
    tint: rgb(0x14, 0x15, 0x18),
    backdrop_overlay: None,
    pill: Color::rgba(0x14, 0x15, 0x18, 0xB0),
    pill_hover: Color::rgba(0x3A, 0x3C, 0x44, 0xE0),
    overlay_text: rgb(0xE8, 0xE8, 0xEC),
    scrollbar: Color::rgba(0xFF, 0xFF, 0xFF, 0x50),
    icon_tint: None,
    scanlines: 0.0,
};

/// 核戦争後の世界を描いたゲームに出てくる、緑の蛍光体のレトロフューチャーな端末画面風。
/// 黒緑の地に緑一色の等幅文字、角張った枠、選択は反転表示、走査線入り。
pub const FALLOUT: Palette = Palette {
    style: Style::Terminal,
    fonts: &["Consolas", "Lucida Console", "Segoe UI"],
    radius: 0.0,
    bg: rgb(0x04, 0x0E, 0x07),
    panel: rgb(0x08, 0x19, 0x0E),
    control: rgb(0x0B, 0x24, 0x14),
    control_hover: rgb(0x12, 0x38, 0x20),
    border: rgb(0x17, 0x6A, 0x3B),
    text: rgb(0x1A, 0xFF, 0x80),
    muted: rgb(0x14, 0xA8, 0x56),
    disabled: rgb(0x0E, 0x4E, 0x2B),
    accent: rgb(0x1A, 0xFF, 0x80),
    accent_hover: rgb(0x8C, 0xFF, 0xBE),
    on_accent: rgb(0x02, 0x10, 0x07),
    error: rgb(0xFF, 0x9A, 0x3C),
    warning: rgb(0xE6, 0xE6, 0x5A),
    solid: rgb(0x03, 0x0B, 0x05),
    tint: rgb(0x01, 0x07, 0x03),
    backdrop_overlay: Some(Color::rgba(0x02, 0x16, 0x09, 0xB4)),
    pill: Color::rgba(0x02, 0x10, 0x07, 0xD8),
    pill_hover: Color::rgba(0x0E, 0x3C, 0x21, 0xF0),
    overlay_text: rgb(0x1A, 0xFF, 0x80),
    scrollbar: Color::rgba(0x1A, 0xFF, 0x80, 0x90),
    icon_tint: Some(rgb(0x1A, 0xFF, 0x80)),
    scanlines: 0.16,
};

/// 選べるスキンの一覧。先頭が既定。
pub static SKINS: [Skin; 2] = [
    Skin {
        id: "standard",
        name: "標準",
        palette: STANDARD,
    },
    Skin {
        id: "fallout",
        name: "Fallout 風 (緑の端末)",
        palette: FALLOUT,
    },
];

/// 既定のスキンの id。
pub fn default_id() -> &'static str {
    SKINS[0].id
}

/// id からスキンの番号を探す。
pub fn index_of(id: &str) -> Option<usize> {
    SKINS.iter().position(|s| s.id == id)
}

thread_local! {
    // UI はすべて1つのスレッドで動くので、スレッドごとの値で足りる
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

/// 使うスキンを切り替える。知らない id なら既定のスキン。
pub fn set(id: &str) {
    CURRENT.with(|c| c.set(index_of(id).unwrap_or(0)));
}

/// いまのスキンの番号 (変わったかどうかの判定用)。
pub fn current_index() -> usize {
    CURRENT.with(Cell::get)
}

pub fn current() -> &'static Skin {
    &SKINS[current_index()]
}

pub fn palette() -> &'static Palette {
    &current().palette
}

/// アイコンを `color` の単色にする (明るさを保つ。暗い部分も見えるよう少し持ち上げる)。
/// 画素は乗算済みの BGRA。
pub fn tinted(p: &Pixels, color: Color) -> Pixels {
    let mut data = p.data.clone();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as f32;
        if a == 0.0 {
            continue;
        }
        // 乗算済みなので、明るさも a 倍されている。元の明るさへ戻してから持ち上げる
        let luma = (0.114 * px[0] as f32 + 0.587 * px[1] as f32 + 0.299 * px[2] as f32) / a;
        let level = (0.3 + 0.7 * luma.clamp(0.0, 1.0)) * a;
        let ch = |c: f32| (c * level).round().clamp(0.0, 255.0) as u8;
        px[0] = ch(color.2);
        px[1] = ch(color.1);
        px[2] = ch(color.0);
    }
    Pixels {
        width: p.width,
        height: p.height,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_default_first() {
        for (i, s) in SKINS.iter().enumerate() {
            assert_eq!(index_of(s.id), Some(i), "重複した id: {}", s.id);
            assert!(!s.palette.fonts.is_empty());
        }
        assert_eq!(default_id(), "standard");
    }

    #[test]
    fn unknown_id_falls_back_to_default() {
        set("fallout");
        assert_eq!(current().id, "fallout");
        set("no-such-skin");
        assert_eq!(current_index(), 0);
    }

    #[test]
    fn tint_keeps_alpha_and_premultiplication() {
        let p = Pixels {
            width: 2,
            height: 1,
            // 白 (不透明) と、半透明の白 (乗算済み)
            data: vec![255, 255, 255, 255, 128, 128, 128, 128],
        };
        let t = tinted(&p, Color::rgba(0, 255, 0, 255));
        assert_eq!(&t.data[0..4], &[0, 255, 0, 255]);
        assert_eq!(t.data[7], 128);
        assert!(t.data[5] <= t.data[7], "乗算済みの値はアルファ以下");
    }
}
