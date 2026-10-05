//! Direct2D / DirectWrite による描画の薄いラッパー。
//!
//! 座標はすべて DIP。レンダーターゲットの DPI をウィンドウの DPI に合わせるので、
//! 物理ピクセルへの変換は Direct2D に任せる。HwndRenderTarget の Present は垂直同期を待つので、
//! アニメーション中は描画のたびに次のフレームを要求するだけで、表示のリフレッシュレートに揃う。

use std::ffi::c_void;

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, GENERIC_READ, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{PCWSTR, Result, w};
use windows_numerics::Matrix3x2;

use crate::platform::icon::Pixels;
use crate::platform::wide::to_wide;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn contains_point(&self, (px, py): (f32, f32)) -> bool {
        self.contains(px, py)
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }

    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self(self.0, self.1, self.2, a)
    }

    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a: self.3 }
    }
}

#[derive(Clone, Copy)]
pub enum TextStyle {
    /// アイテム名 (左寄せ・縦中央・末尾省略)
    Item,
    ItemBold,
    /// 上部と下部の小さな文字 (中央寄せ)
    Small,
    /// 記号フォントのアイコン (中央寄せ)
    Glyph,
    /// 設定画面などの見出し (左寄せ・やや大きく太い)
    Heading,
    /// 項目名など (左寄せ・縦中央・末尾省略)
    Label,
    /// ボタンや値の表示 (中央寄せ・縦中央・末尾省略)
    Value,
    /// 補足説明 (左寄せ・上揃え・折り返し)
    Caption,
}

pub struct Gfx {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    target: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    item: IDWriteTextFormat,
    item_bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    caption: IDWriteTextFormat,
    /// レンダーターゲットを作り直すたびに増える。ビットマップはこの世代のものだけ有効
    pub generation: u64,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        // SAFETY: COM/D2D/DWrite のファクトリ生成
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

            let ui = w!("Segoe UI");
            let (left, center) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_CENTER);
            let item = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 13.0, left, false)?;
            let item_bold = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_BOLD, 13.0, left, false)?;
            let small = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 12.0, center, false)?;
            let glyph_family = if has_font(&dwrite, w!("Segoe Fluent Icons")) {
                w!("Segoe Fluent Icons")
            } else {
                w!("Segoe MDL2 Assets")
            };
            let glyph = text_format(&dwrite, glyph_family, DWRITE_FONT_WEIGHT_NORMAL, 14.0, center, false)?;
            let heading = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 17.0, left, false)?;
            let label = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 13.0, left, false)?;
            let value = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 13.0, center, false)?;
            let caption = text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 12.0, left, true)?;
            for f in [&item, &item_bold, &label, &value, &small] {
                let sign = dwrite.CreateEllipsisTrimmingSign(f)?;
                f.SetTrimming(
                    &DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 },
                    &sign,
                )?;
            }
            Ok(Self {
                d2d,
                dwrite,
                wic,
                target: None,
                brush: None,
                item,
                item_bold,
                small,
                glyph,
                heading,
                label,
                value,
                caption,
                generation: 0,
            })
        }
    }

    fn format(&self, style: TextStyle) -> &IDWriteTextFormat {
        match style {
            TextStyle::Item => &self.item,
            TextStyle::ItemBold => &self.item_bold,
            TextStyle::Small => &self.small,
            TextStyle::Glyph => &self.glyph,
            TextStyle::Heading => &self.heading,
            TextStyle::Label => &self.label,
            TextStyle::Value => &self.value,
            TextStyle::Caption => &self.caption,
        }
    }

    /// 文字列を `max_width` 以内に配置した時の (幅, 高さ)。折り返す書式では高さが伸びる。
    pub fn measure(&self, s: &str, style: TextStyle, max_width: f32) -> (f32, f32) {
        let text: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: テキストレイアウトの生成と計測のみ
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&text, self.format(style), max_width, 10_000.0) else {
                return (0.0, 0.0);
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return (0.0, 0.0);
            }
            (m.widthIncludingTrailingWhitespace, m.height)
        }
    }

    /// 画像ファイルの寸法 (ピクセル)。読めなければ None。
    pub fn image_size(&self, path: &str) -> Option<(u32, u32)> {
        let wide = to_wide(path);
        // SAFETY: WIC による寸法の取得のみ
        unsafe {
            let decoder = self
                .wic
                .CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let (mut w, mut h) = (0, 0);
            frame.GetSize(&mut w, &mut h).ok()?;
            Some((w, h))
        }
    }

    /// レンダーターゲットが無ければ作る。
    pub fn ensure_target(&mut self, hwnd: HWND) -> Result<()> {
        if self.target.is_some() {
            return Ok(());
        }
        // SAFETY: 有効なウィンドウに対するレンダーターゲット生成
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc)?;
            let dpi = GetDpiForWindow(hwnd).max(96) as f32;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                ..Default::default()
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = self.d2d.CreateHwndRenderTarget(&props, &hwnd_props)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.brush = Some(target.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)?);
            self.target = Some(target);
            self.generation += 1;
        }
        Ok(())
    }

    pub fn resize(&self, width: u32, height: u32) {
        if let Some(t) = &self.target {
            // SAFETY: 有効なレンダーターゲット
            let _ = unsafe { t.Resize(&D2D_SIZE_U { width, height }) };
        }
    }

    pub fn set_dpi(&self, dpi: u32) {
        if let Some(t) = &self.target {
            // SAFETY: 有効なレンダーターゲット
            unsafe { t.SetDpi(dpi as f32, dpi as f32) };
        }
    }

    pub fn begin(&self, clear: Color) {
        if let Some(t) = &self.target {
            // SAFETY: 描画開始
            unsafe {
                t.BeginDraw();
                t.SetTransform(&Matrix3x2::identity());
                t.Clear(Some(&clear.d2d()));
            }
        }
    }

    /// 描画を確定する。デバイス消失時はターゲットを破棄し、次回に作り直させる。
    pub fn end(&mut self) {
        if let Some(t) = &self.target {
            // SAFETY: 描画終了
            if let Err(e) = unsafe { t.EndDraw(None, None) } {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.brush = None;
                }
            }
        }
    }

    fn brush(&self, color: Color) -> Option<&ID2D1SolidColorBrush> {
        let b = self.brush.as_ref()?;
        // SAFETY: 有効なブラシ
        unsafe { b.SetColor(&color.d2d()) };
        Some(b)
    }

    pub fn fill_round(&self, r: Rect, radius: f32, color: Color) {
        if let (Some(t), Some(b)) = (&self.target, self.brush(color)) {
            let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
            // SAFETY: 描画中のターゲットへの描画
            unsafe { t.FillRoundedRectangle(&rr, b) };
        }
    }

    pub fn fill_rect(&self, r: Rect, color: Color) {
        if let (Some(t), Some(b)) = (&self.target, self.brush(color)) {
            // SAFETY: 描画中のターゲットへの描画
            unsafe { t.FillRectangle(&r.d2d(), b) };
        }
    }

    pub fn stroke_round(&self, r: Rect, radius: f32, color: Color, width: f32) {
        if let (Some(t), Some(b)) = (&self.target, self.brush(color)) {
            // 線の中心が矩形の内側に来るようにする
            let rr = D2D1_ROUNDED_RECT { rect: r.inset(width / 2.0, width / 2.0).d2d(), radiusX: radius, radiusY: radius };
            // SAFETY: 描画中のターゲットへの描画
            unsafe { t.DrawRoundedRectangle(&rr, b, width, None) };
        }
    }

    pub fn text(&self, s: &str, r: Rect, color: Color, style: TextStyle) {
        let format = self.format(style);
        if let (Some(t), Some(b)) = (&self.target, self.brush(color)) {
            let text: Vec<u16> = s.encode_utf16().collect();
            // SAFETY: 描画中のターゲットへの描画
            unsafe {
                t.DrawText(&text, format, &r.d2d(), b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL)
            };
        }
    }

    /// 上部・下部の小さな文字の描画幅 (DIP)。
    pub fn measure_small(&self, s: &str) -> f32 {
        self.measure(s, TextStyle::Small, 10_000.0).0
    }

    /// 中心 (cx, cy) を軸に `degrees` 回転させて描く範囲を開始する。
    pub fn rotate(&self, degrees: f32, cx: f32, cy: f32) {
        if let Some(t) = &self.target {
            let (s, c) = degrees.to_radians().sin_cos();
            let m = Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: cx - c * cx + s * cy, M32: cy - s * cx - c * cy };
            // SAFETY: 変換行列の設定
            unsafe { t.SetTransform(&m) };
        }
    }

    pub fn reset_transform(&self) {
        if let Some(t) = &self.target {
            // SAFETY: 変換行列の設定
            unsafe { t.SetTransform(&Matrix3x2::identity()) };
        }
    }

    pub fn push_clip(&self, r: Rect) {
        if let Some(t) = &self.target {
            // SAFETY: 描画中のターゲットへのクリップ設定 (pop_clip と対で使う)
            unsafe { t.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn pop_clip(&self) {
        if let Some(t) = &self.target {
            // SAFETY: push_clip と対
            unsafe { t.PopAxisAlignedClip() };
        }
    }

    pub fn bitmap(&self, bmp: &ID2D1Bitmap, r: Rect, opacity: f32) {
        if let Some(t) = &self.target {
            // SAFETY: 描画中のターゲットへの描画
            unsafe {
                t.DrawBitmap(bmp, Some(&r.d2d()), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None)
            };
        }
    }

    /// 画像を中央基準で `r` 全体を覆うよう拡大・切り抜きして描く。
    pub fn bitmap_cover(&self, bmp: &ID2D1Bitmap, r: Rect) {
        let Some(t) = &self.target else { return };
        // SAFETY: 描画中のターゲットへの描画
        unsafe {
            let size = bmp.GetSize();
            if size.width <= 0.0 || size.height <= 0.0 {
                return;
            }
            let scale = (r.w / size.width).max(r.h / size.height);
            let (sw, sh) = (r.w / scale, r.h / scale);
            let src = D2D_RECT_F {
                left: (size.width - sw) / 2.0,
                top: (size.height - sh) / 2.0,
                right: (size.width + sw) / 2.0,
                bottom: (size.height + sh) / 2.0,
            };
            t.DrawBitmap(bmp, Some(&r.d2d()), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, Some(&src));
        }
    }

    /// ビットマップの一部 `src` (x, y, w, h: ビットマップの画素) を `dest` へ引き伸ばして描く。
    /// `src` がビットマップの外にはみ出す分は描かない (描き先もその割合で縮める)。
    pub fn bitmap_part(&self, bmp: &ID2D1Bitmap, dest: Rect, src: (f32, f32, f32, f32)) {
        let Some(t) = &self.target else { return };
        let (sx, sy, sw, sh) = src;
        if sw <= 0.0 || sh <= 0.0 {
            return;
        }
        // SAFETY: 描画中のターゲットへの描画
        unsafe {
            let size = bmp.GetSize();
            let (x0, y0) = (sx.max(0.0), sy.max(0.0));
            let (x1, y1) = ((sx + sw).min(size.width), (sy + sh).min(size.height));
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let (kx, ky) = (dest.w / sw, dest.h / sh);
            let d = Rect::new(dest.x + (x0 - sx) * kx, dest.y + (y0 - sy) * ky, (x1 - x0) * kx, (y1 - y0) * ky);
            let s = D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 };
            t.DrawBitmap(bmp, Some(&d.d2d()), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, Some(&s));
        }
    }

    /// BGRA (乗算済み) の画素からビットマップを作る。
    pub fn create_bitmap(&self, p: &Pixels) -> Option<ID2D1Bitmap> {
        let t = self.target.as_ref()?;
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: 画素バッファは width*height*4 バイト
        unsafe {
            t.CreateBitmap(
                D2D_SIZE_U { width: p.width as u32, height: p.height as u32 },
                Some(p.data.as_ptr() as *const c_void),
                (p.width * 4) as u32,
                &props,
            )
            .ok()
        }
    }

    /// 画像ファイルを読み込む。読めなければ None (背景なしとして扱う)。
    pub fn load_image(&self, path: &str) -> Option<ID2D1Bitmap> {
        let t = self.target.as_ref()?;
        let wide = to_wide(path);
        // SAFETY: WIC による読込とビットマップ化
        unsafe {
            let decoder = self
                .wic
                .CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = self.wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
                .ok()?;
            t.CreateBitmapFromWicBitmap(&converter, None).ok()
        }
    }
}

unsafe fn text_format(
    dwrite: &IDWriteFactory,
    family: PCWSTR,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
    align: DWRITE_TEXT_ALIGNMENT,
    wrap: bool,
) -> Result<IDWriteTextFormat> {
    // SAFETY: テキスト書式の生成
    unsafe {
        let f = dwrite.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("ja-jp"))?;
        if wrap {
            // 折り返す文章は上揃え
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        } else {
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        f.SetTextAlignment(align)?;
        Ok(f)
    }
}

fn has_font(dwrite: &IDWriteFactory, family: PCWSTR) -> bool {
    // SAFETY: システムフォントの検索のみ
    unsafe {
        let mut collection = None;
        if dwrite.GetSystemFontCollection(&mut collection, false).is_err() {
            return false;
        }
        let Some(collection) = collection else { return false };
        let (mut index, mut exists) = (0u32, windows::core::BOOL(0));
        collection.FindFamilyName(family, &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}
