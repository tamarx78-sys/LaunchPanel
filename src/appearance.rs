//! メインウィンドウの背景 (単色・壁紙ぼかし・アクリル・背景画像) と暗さの重ね。
//!
//! 背景画像があれば最優先し、ぼかしの強さ (0 ならそのまま) と暗さを掛けて敷く。
//! 無ければ背景の種類に従う。アクリルは DWM のシステム背景を使い、使えない環境
//! (Windows 10・古い Windows 11) では壁紙ぼかしで代用する。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{s, w};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::backdrop::{self, BLUR_LIMIT, WallpaperCanvas};
use crate::config::{Backdrop, Settings};
use crate::platform::icon::Pixels;
use crate::render::{Color, Gfx, Rect};

/// 背景の描き方を決める値。設定画面の試し表示中は編集中の値になる。
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub image: String,
    pub backdrop: Backdrop,
    pub blur: i32,
    pub tint: i32,
}

impl Look {
    pub fn of(s: &Settings) -> Self {
        Self { image: s.background_image.trim().to_owned(), backdrop: s.backdrop, blur: s.blur, tint: s.tint }
    }

    fn wants_acrylic(&self) -> bool {
        self.image.is_empty() && self.backdrop == Backdrop::Acrylic
    }
}

const SOLID: Color = Color::rgba(0x22, 0x23, 0x28, 0xFF);
const TINT: Color = Color::rgba(0x14, 0x15, 0x18, 0xFF);

#[derive(Default)]
pub struct BackdropView {
    acrylic: bool,
    wallpaper: Option<WallpaperCanvas>,
    /// (世代, 壁紙のキー, ぼかしの強さ, ビットマップ, キャンバス画素あたりのビットマップ画素)
    wallpaper_bitmap: Option<(u64, String, i32, ID2D1Bitmap, f32)>,
    /// 背景画像を縮小して読んだもの (ぼかし用)
    image_source: Option<(String, Option<Pixels>)>,
    /// (世代, パス, ぼかしの強さ, ビットマップ)。強さ 0 は元の解像度のまま
    image_bitmap: Option<(u64, String, i32, Option<ID2D1Bitmap>)>,
}

impl BackdropView {
    /// アクリルの有無を DWM に設定する。使えなければ false を返し、壁紙ぼかしで代用する。
    pub fn update_mode(&mut self, hwnd: HWND, look: &Look) {
        let want = look.wants_acrylic();
        if want == self.acrylic {
            return;
        }
        // DWMWA_SYSTEMBACKDROP_TYPE (公開 API) は枠なしのポップアップウィンドウには効かないので、
        // 多くのアプリが使っている SetWindowCompositionAttribute のアクセント (アクリル) を使う
        if !set_accent(hwnd, want) {
            if want {
                crate::log::write("アクリル背景を使えないため、壁紙ぼかしで代用します。");
            }
            return;
        }
        self.acrylic = want;
    }

    /// 描画開始時の塗り色。アクリル中は透明にして DWM の背景を見せる。
    pub fn clear_color(&self) -> Color {
        if self.acrylic { Color(0.0, 0.0, 0.0, 0.0) } else { SOLID }
    }

    /// 壁紙・ディスプレイ構成が変わったら作り直させる。
    pub fn invalidate_wallpaper(&mut self) {
        self.wallpaper = None;
        self.wallpaper_bitmap = None;
    }

    /// 背景が壁紙ぼかしか (ウィンドウを動かしたら描き直しが必要か)。
    pub fn follows_position(&self, look: &Look) -> bool {
        look.image.is_empty() && (look.backdrop == Backdrop::Wallpaper || (look.backdrop == Backdrop::Acrylic && !self.acrylic))
    }

    pub fn draw(&mut self, gfx: &Gfx, hwnd: HWND, look: &Look, full: Rect) {
        if !look.image.is_empty() {
            if let Some(b) = self.image(gfx, look) {
                gfx.bitmap_cover(&b, full);
            }
        } else if self.follows_position(look) {
            self.draw_wallpaper(gfx, hwnd, look.blur, full);
        }
        if look.tint > 0 {
            gfx.fill_rect(full, TINT.with_alpha(look.tint as f32 / 100.0 * 0.92));
        }
    }

    fn image(&mut self, gfx: &Gfx, look: &Look) -> Option<ID2D1Bitmap> {
        let generation = gfx.generation;
        let fresh = self.image_bitmap.as_ref().is_some_and(|(g, p, b, _)| *g == generation && *p == look.image && *b == look.blur);
        if !fresh {
            let bitmap = if look.blur == 0 {
                gfx.load_image(&look.image)
            } else {
                if self.image_source.as_ref().is_none_or(|(p, _)| *p != look.image) {
                    self.image_source = Some((look.image.clone(), backdrop::decode_limited(&look.image, BLUR_LIMIT)));
                }
                let source = self.image_source.as_ref().and_then(|(_, s)| s.as_ref());
                source.and_then(|s| gfx.create_bitmap(&backdrop::blurred(s, look.blur)))
            };
            if bitmap.is_none() {
                crate::log::write(&format!("背景画像を読み込めません: {}", look.image));
            }
            self.image_bitmap = Some((generation, look.image.clone(), look.blur, bitmap));
        }
        self.image_bitmap.as_ref().and_then(|(_, _, _, b)| b.clone())
    }

    /// 壁紙のうち、ウィンドウの裏にあたる部分をぼかして描く。
    fn draw_wallpaper(&mut self, gfx: &Gfx, hwnd: HWND, blur: i32, full: Rect) {
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut win = RECT::default();
        // SAFETY: モニターとウィンドウの矩形の取得
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
            let _ = GetWindowRect(hwnd, &mut win);
        }
        let monitor = info.rcMonitor;
        if self.wallpaper.as_ref().is_none_or(|c| c.monitor != monitor) {
            self.wallpaper = backdrop::wallpaper_canvas(monitor, BLUR_LIMIT);
            self.wallpaper_bitmap = None;
        }
        let Some(canvas) = &self.wallpaper else { return };
        let generation = gfx.generation;
        let fresh = self
            .wallpaper_bitmap
            .as_ref()
            .is_some_and(|(g, k, b, _, _)| *g == generation && *k == canvas.key && *b == blur);
        if !fresh {
            let pixels = backdrop::blurred(&canvas.pixels, blur);
            let ratio = pixels.width as f32 / canvas.pixels.width as f32;
            self.wallpaper_bitmap = gfx.create_bitmap(&pixels).map(|b| (generation, canvas.key.clone(), blur, b, ratio));
        }
        let Some((_, _, _, bitmap, ratio)) = &self.wallpaper_bitmap else { return };

        // ウィンドウの位置 (物理座標) → ビットマップ上の位置
        let f = canvas.factor * ratio;
        let src = (
            (win.left - monitor.left) as f32 * f,
            (win.top - monitor.top) as f32 * f,
            (win.right - win.left) as f32 * f,
            (win.bottom - win.top) as f32 * f,
        );
        gfx.bitmap_part(bitmap, full, src);
    }
}

/// SetWindowCompositionAttribute でアクリルのアクセントを付け外しする。
/// user32 の非公開関数なので、見つからなければ false (壁紙ぼかしで代用する)。
fn set_accent(hwnd: HWND, acrylic: bool) -> bool {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        /// 0xAABBGGRR。暗さは自前で重ねるので、ほぼ透明にする
        gradient_color: u32,
        animation_id: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attribute: u32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;

    // SAFETY: user32 から関数を取り出して呼ぶ。構造体は呼び出し中有効
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return false };
        let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return false };
        let set: SetWca = std::mem::transmute(f);
        let mut policy = AccentPolicy {
            state: if acrylic { ACCENT_ENABLE_ACRYLICBLURBEHIND } else { ACCENT_DISABLED },
            flags: 0,
            gradient_color: 0x0118_1514,
            animation_id: 0,
        };
        let mut data = CompositionData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut _,
            size: size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data) != 0
    }
}
