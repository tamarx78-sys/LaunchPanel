//! 設定画面。文字や数値の直接入力欄を置かず、マウス操作だけで完結させる。
//!
//! - 数値: ドラッグで変える欄 (1列の幅) とスライダー (濃さ・透過率)
//! - 画像: ドロップ欄・ファイル選択ダイアログ・クリア
//! - 色: 色見本から開く選択パネル (彩度・明度の平面、色相の帯、色見本)
//! - 修飾キー: トグルボタン (最後の1つは外せない)、文字キー: 一覧から選ぶ
//!
//! 編集中の値はこの画面の中だけで持ち、OK で本体へ渡す。影の設定だけは操作中から本体へ
//! 試し表示し、キャンセルでは本体が保存済みの値へ戻す。

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect, ValidateRect};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragFinish, DragQueryFileW, FileOpenDialog, HDROP, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::appearance::Look;
use crate::color::{self, Hsv};
use crate::config::{
    Backdrop, ItemButtonSettings, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, Rgb, Settings,
};
use crate::dialog::{self, Dialog};
use crate::platform::icon::Pixels;
use crate::platform::shadow;
use crate::render::{Color, Gfx, Rect, TextStyle};
use crate::ui::{self, State};

/// 設定画面が閉じた (wParam: OK なら 1)。結果は [`take_result`] で受け取る。
pub const WM_APP_SETTINGS: u32 = WM_APP + 14;
/// 背景の試し表示を更新した。値は [`preview_look`] で受け取る。
pub const WM_APP_PREVIEW: u32 = WM_APP + 16;

const MAX_IMAGE: u32 = 2000;
const MARGIN: f32 = 24.0;
const ROW: f32 = 44.0;
const CTRL_H: f32 = 30.0;
const FOOTER: f32 = 60.0;
const PICKER_W: f32 = 268.0;
const SV_H: f32 = 150.0;

thread_local! {
    static RESULT: RefCell<Option<Settings>> = const { RefCell::new(None) };
    static PREVIEW: RefCell<Option<Look>> = const { RefCell::new(None) };
}

/// Windows の「透明効果」が有効か。無効だとアクリルは単色になる。
fn transparency_enabled() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut value = 1u32;
    let mut size = 4u32;
    // SAFETY: 出力バッファの大きさを渡している
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        );
    }
    value != 0
}

/// 設定画面で編集中の背景 (本体の試し表示用)。
pub fn preview_look() -> Option<Look> {
    PREVIEW.with(|p| p.borrow().clone())
}

/// OK で閉じた時の設定を受け取る。
pub fn take_result() -> Option<Settings> {
    RESULT.with(|r| r.borrow_mut().take())
}

/// 設定画面を開く。`icon` はプレビュー用のアイコン。
pub fn open(owner: HWND, current: &Settings, icon: Option<&Pixels>) -> bool {
    let current = current.clone();
    let icon = icon.cloned();
    dialog::open(owner, "LaunchPanel の設定", 520.0, 780.0, move |hwnd| {
        Box::new(SettingsDialog::new(hwnd, owner, current, icon))
    })
    .is_some()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum T {
    ColumnWidth,
    Shadow,
    ShadowOpacity,
    DropZone,
    Browse,
    Clear,
    BackdropKind(usize),
    Blur,
    Tint,
    BgColor,
    Transparency,
    TextColor,
    Bold,
    Preview,
    Reset,
    Modifier(usize),
    Key,
    Desktop,
    Ok,
    Cancel,
    // 色の選択パネル (クライアント座標)
    PickSv,
    PickHue,
    Preset(usize),
    PickClose,
    PickPanel,
}

#[derive(Clone, Copy, PartialEq)]
enum ColorTarget {
    Background,
    Text,
}

struct Picker {
    target: ColorTarget,
    hsv: Hsv,
    /// 色見本の位置 (クライアント座標)。パネルはこの近くに開く
    anchor: Rect,
    sv: Option<(u64, f32, ID2D1Bitmap)>,
    hue: Option<(u64, ID2D1Bitmap)>,
}

struct Drag {
    target: T,
    start_x: f32,
    start_value: i32,
}

/// レイアウト結果。位置はコンテンツ座標 (スクロール前)。
#[derive(Default)]
struct Layout {
    controls: Vec<(T, Rect)>,
    texts: Vec<(String, Rect, Color, TextStyle)>,
    height: f32,
}

struct SettingsDialog {
    hwnd: HWND,
    owner: HWND,
    draft: Settings,
    gfx: Option<Gfx>,
    icon: Option<Pixels>,
    icon_bitmap: Option<(u64, ID2D1Bitmap)>,
    thumbnail: Option<(u64, String, Option<ID2D1Bitmap>)>,
    background_message: Option<(String, bool)>,
    hotkey_message: Option<String>,
    width: f32,
    height: f32,
    scroll: f32,
    hover: Option<T>,
    pressed: Option<T>,
    drag: Option<Drag>,
    picker: Option<Picker>,
    tracking_leave: bool,
    finished: bool,
}

impl SettingsDialog {
    fn new(hwnd: HWND, owner: HWND, current: Settings, icon: Option<Pixels>) -> Self {
        let gfx = Gfx::new().ok();
        let s = dialog::scale(hwnd);
        let mut rc = windows::Win32::Foundation::RECT::default();
        // SAFETY: 自分のウィンドウのクライアント領域の取得
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        Self {
            hwnd,
            owner,
            draft: current,
            gfx,
            icon,
            icon_bitmap: None,
            thumbnail: None,
            background_message: None,
            hotkey_message: None,
            width: (rc.right - rc.left) as f32 / s,
            height: (rc.bottom - rc.top) as f32 / s,
            scroll: 0.0,
            hover: None,
            pressed: None,
            drag: None,
            picker: None,
            tracking_leave: false,
            finished: false,
        }
    }

    fn invalidate(&self) {
        // SAFETY: 再描画要求
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // ───────────── レイアウト ─────────────

    fn layout(&self) -> Layout {
        let mut l = Layout::default();
        let Some(gfx) = &self.gfx else { return l };
        let cw = (self.width - MARGIN * 2.0).max(100.0);
        let right = MARGIN + cw;
        let mut y = 16.0;

        let heading = |l: &mut Layout, y: &mut f32, text: &str| {
            l.texts.push((
                text.into(),
                Rect::new(MARGIN, *y, cw, 30.0),
                ui::TEXT,
                TextStyle::Heading,
            ));
            *y += 38.0;
        };
        let caption = |l: &mut Layout, y: &mut f32, text: &str, color: Color| {
            let (_, h) = gfx.measure(text, TextStyle::Caption, cw);
            l.texts.push((
                text.into(),
                Rect::new(MARGIN, *y, cw, h + 2.0),
                color,
                TextStyle::Caption,
            ));
            *y += h + 12.0;
        };
        // 左に項目名、右端に操作部品を置く1行
        let row = |l: &mut Layout, y: &mut f32, label: &str, t: T, w: f32, h: f32| -> Rect {
            l.texts.push((
                label.into(),
                Rect::new(MARGIN, *y, cw - w - 12.0, ROW),
                ui::TEXT,
                TextStyle::Label,
            ));
            let r = Rect::new(right - w, *y + (ROW - h) / 2.0, w, h);
            l.controls.push((t, r));
            *y += ROW;
            r
        };
        // スライダーと右側の値表示
        let slider_row = |l: &mut Layout, y: &mut f32, label: &str, t: T, value: i32| {
            let r = row(l, y, label, t, 250.0, 24.0);
            let slider = Rect::new(r.x, r.y, r.w - 56.0, r.h);
            l.controls.last_mut().unwrap().1 = slider;
            l.texts.push((
                format!("{value}%"),
                Rect::new(r.x + r.w - 48.0, r.y, 48.0, r.h),
                ui::MUTED,
                TextStyle::Value,
            ));
        };
        let color_row = |l: &mut Layout, y: &mut f32, label: &str, t: T, c: Rgb| {
            let r = row(l, y, label, t, 48.0, 28.0);
            let text = format!("R {}  G {}  B {}", c.0, c.1, c.2);
            l.texts.push((
                text,
                Rect::new(r.x - 150.0, r.y, 140.0, r.h),
                ui::MUTED,
                TextStyle::Value,
            ));
        };

        // 1. ウィンドウ
        heading(&mut l, &mut y, "ウィンドウ");
        row(&mut l, &mut y, "1列の幅", T::ColumnWidth, 160.0, CTRL_H);
        row(
            &mut l,
            &mut y,
            "ウィンドウに影を表示",
            T::Shadow,
            46.0,
            24.0,
        );
        slider_row(
            &mut l,
            &mut y,
            "影の濃さ",
            T::ShadowOpacity,
            self.draft.window.shadow_opacity,
        );
        caption(
            &mut l,
            &mut y,
            "1列の幅は左右にドラッグして変えます (Shift で 1px 単位)。影の設定は操作中から本体に反映され、キャンセルで元に戻ります。",
            ui::MUTED,
        );
        y += 12.0;

        // 2. 背景
        heading(&mut l, &mut y, "背景");
        l.controls
            .push((T::DropZone, Rect::new(MARGIN, y, cw, 120.0)));
        y += 130.0;
        l.controls
            .push((T::Browse, Rect::new(MARGIN, y, 100.0, CTRL_H)));
        l.controls
            .push((T::Clear, Rect::new(MARGIN + 108.0, y, 100.0, CTRL_H)));
        y += CTRL_H + 10.0;
        match &self.background_message {
            Some((m, true)) => caption(&mut l, &mut y, m, ui::ERROR),
            Some((m, false)) => caption(&mut l, &mut y, m, ui::WARNING),
            None => caption(
                &mut l,
                &mut y,
                &format!(
                    "画像ファイルをドロップするか、参照から選びます。幅・高さとも {MAX_IMAGE}px 未満の画像を指定できます。"
                ),
                ui::MUTED,
            ),
        }
        // 画像がない時の背景 (なし / 壁紙ぼかし / アクリル)
        l.texts.push((
            "画像がない時の背景".into(),
            Rect::new(MARGIN, y, cw, ROW),
            ui::TEXT,
            TextStyle::Label,
        ));
        let mut x = right;
        for (i, w) in [86.0, 100.0, 60.0].into_iter().enumerate().rev() {
            x -= w;
            l.controls.push((
                T::BackdropKind(i),
                Rect::new(x, y + (ROW - CTRL_H) / 2.0, w, CTRL_H),
            ));
            x -= 6.0;
        }
        y += ROW;
        slider_row(&mut l, &mut y, "ぼかしの強さ", T::Blur, self.draft.blur);
        slider_row(&mut l, &mut y, "暗さ", T::Tint, self.draft.tint);
        caption(
            &mut l,
            &mut y,
            "ぼかしの強さは壁紙ぼかしと背景画像に効きます。アクリルは裏のウィンドウも透けて見えますが、ぼかしの強さは Windows が決めます (Windows 11 のみ。使えない環境では壁紙ぼかしになります)。",
            ui::MUTED,
        );
        if self.draft.backdrop == Backdrop::Acrylic && !transparency_enabled() {
            caption(
                &mut l,
                &mut y,
                "Windows の「透明効果」がオフのため、アクリルは単色で表示されます (設定 > 個人用設定 > 色 > 透明効果)。",
                ui::WARNING,
            );
        }
        y += 12.0;

        // 3. ボタン
        let b = &self.draft.item_button;
        heading(&mut l, &mut y, "ボタン");
        color_row(&mut l, &mut y, "背景色", T::BgColor, b.background_color);
        slider_row(
            &mut l,
            &mut y,
            "透過率 (0% = 不透明)",
            T::Transparency,
            b.transparency,
        );
        color_row(&mut l, &mut y, "テキスト色", T::TextColor, b.text_color);
        row(&mut l, &mut y, "太字", T::Bold, 46.0, 24.0);
        y += 6.0;
        l.texts.push((
            "プレビュー (ポインターを重ねると強調表示)".into(),
            Rect::new(MARGIN, y, cw, 20.0),
            ui::MUTED,
            TextStyle::Caption,
        ));
        y += 24.0;
        l.controls
            .push((T::Preview, Rect::new(MARGIN, y, cw, 76.0)));
        y += 86.0;
        l.controls
            .push((T::Reset, Rect::new(MARGIN, y, 150.0, CTRL_H)));
        y += CTRL_H + 22.0;

        // 4. ホットキー
        heading(&mut l, &mut y, "ホットキー");
        let mut x = MARGIN;
        for (i, w) in [62.0, 62.0, 66.0, 62.0].into_iter().enumerate() {
            l.controls.push((
                T::Modifier(i),
                Rect::new(x, y + (ROW - CTRL_H) / 2.0, w, CTRL_H),
            ));
            x += w + 6.0;
        }
        l.texts.push((
            "+".into(),
            Rect::new(x, y, 20.0, ROW),
            ui::MUTED,
            TextStyle::Value,
        ));
        l.controls.push((
            T::Key,
            Rect::new(x + 26.0, y + (ROW - CTRL_H) / 2.0, 72.0, CTRL_H),
        ));
        y += ROW + 4.0;
        match &self.hotkey_message {
            Some(m) => caption(&mut l, &mut y, m, ui::ERROR),
            None => caption(
                &mut l,
                &mut y,
                "ホットキーの変更は次回起動時から有効になります。Win は現在のバージョンでは変更できません。",
                ui::MUTED,
            ),
        }
        y += 12.0;

        // 5. 実験的機能
        heading(&mut l, &mut y, "実験的機能");
        row(
            &mut l,
            &mut y,
            "デスクトップ空白のダブルクリックで表示",
            T::Desktop,
            46.0,
            24.0,
        );
        caption(
            &mut l,
            &mut y,
            "Windows の更新、Explorer の構成、他のアプリのフックやオーバーレイによって、動作が影響を受ける場合があります。",
            ui::MUTED,
        );
        l.height = y + 8.0;
        l
    }

    fn footer_controls(&self) -> [(T, Rect); 2] {
        let y = self.height - FOOTER + (FOOTER - 32.0) / 2.0;
        [
            (
                T::Ok,
                Rect::new(self.width - MARGIN - 108.0 * 2.0 + 8.0, y, 100.0, 32.0),
            ),
            (
                T::Cancel,
                Rect::new(self.width - MARGIN - 100.0, y, 100.0, 32.0),
            ),
        ]
    }

    fn content_view(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, (self.height - FOOTER).max(0.0))
    }

    fn max_scroll(&self, layout: &Layout) -> f32 {
        (layout.height - self.content_view().h).max(0.0)
    }

    /// コンテンツ座標の矩形を、スクロールを反映したクライアント座標へ。
    fn view(&self, r: Rect) -> Rect {
        Rect::new(r.x, r.y - self.scroll, r.w, r.h)
    }

    fn picker_rects(&self) -> Option<Vec<(T, Rect)>> {
        let p = self.picker.as_ref()?;
        let h = 12.0 + SV_H + 10.0 + 16.0 + 12.0 + 24.0 * 2.0 + 8.0 + 12.0 + 32.0 + 12.0;
        let mut x = (p.anchor.x + p.anchor.w - PICKER_W).max(8.0);
        x = x.min(self.width - PICKER_W - 8.0).max(8.0);
        let below = p.anchor.y + p.anchor.h + 6.0;
        let y = if below + h <= self.height - 8.0 {
            below
        } else {
            (p.anchor.y - 6.0 - h).max(8.0)
        };
        let inner = PICKER_W - 24.0;
        let mut v = vec![(T::PickPanel, Rect::new(x, y, PICKER_W, h))];
        v.push((T::PickSv, Rect::new(x + 12.0, y + 12.0, inner, SV_H)));
        v.push((
            T::PickHue,
            Rect::new(x + 12.0, y + 22.0 + SV_H, inner, 16.0),
        ));
        let cell = (inner - 5.0 * 8.0) / 6.0;
        let py = y + 22.0 + SV_H + 16.0 + 12.0;
        for i in 0..color::PRESETS.len() {
            let (c, r) = (i % 6, i / 6);
            v.push((
                T::Preset(i),
                Rect::new(
                    x + 12.0 + c as f32 * (cell + 8.0),
                    py + r as f32 * 32.0,
                    cell,
                    24.0,
                ),
            ));
        }
        v.push((
            T::PickClose,
            Rect::new(x + PICKER_W - 12.0 - 90.0, y + h - 12.0 - 32.0, 90.0, 32.0),
        ));
        Some(v)
    }

    fn hit(&self, x: f32, y: f32) -> Option<T> {
        if let Some(rects) = self.picker_rects() {
            // 色の選択パネルを開いている間は、パネル内だけを操作対象にする (外は None で閉じる)
            if !rects[0].1.contains(x, y) {
                return None;
            }
            return rects[1..]
                .iter()
                .find(|(_, r)| r.contains(x, y))
                .map(|(t, _)| *t)
                .or(Some(T::PickPanel));
        }
        if let Some((t, _)) = self
            .footer_controls()
            .iter()
            .find(|(_, r)| r.contains(x, y))
        {
            return Some(*t);
        }
        if !self.content_view().contains(x, y) {
            return None;
        }
        self.layout()
            .controls
            .iter()
            .find(|(_, r)| self.view(*r).contains(x, y))
            .map(|(t, _)| *t)
    }

    fn enabled(&self, t: T) -> bool {
        match t {
            T::ShadowOpacity => self.draft.window.shadow,
            // ぼかしは壁紙ぼかしか背景画像の時だけ効く
            T::Blur => {
                !self.draft.background_image.is_empty()
                    || self.draft.backdrop == Backdrop::Wallpaper
            }
            T::Clear => !self.draft.background_image.is_empty(),
            T::Modifier(3) => false, // Win は現在のバージョンでは変更不可
            _ => true,
        }
    }

    // ───────────── 値の変更 ─────────────

    /// 編集中の背景を本体へ試し表示する。
    fn preview_look(&self) {
        let d = &self.draft;
        let look = Look {
            image: d.background_image.clone(),
            backdrop: d.backdrop,
            blur: d.blur,
            tint: d.tint,
        };
        PREVIEW.with(|p| *p.borrow_mut() = Some(look));
        // SAFETY: 本体への通知
        unsafe {
            let _ = PostMessageW(Some(self.owner), WM_APP_PREVIEW, WPARAM(0), LPARAM(0));
        }
    }

    fn preview_shadow(&self) {
        let w = &self.draft.window;
        // SAFETY: 本体ウィンドウへの影の設定 (同じ UI スレッド)
        unsafe {
            shadow::lp_set_window_shadow(self.owner.0 as isize, w.shadow as i32, w.shadow_opacity)
        };
    }

    fn set_slider(&mut self, t: T, x: f32) {
        let Some(r) = self
            .layout()
            .controls
            .iter()
            .find(|(c, _)| *c == t)
            .map(|(_, r)| self.view(*r))
        else {
            return;
        };
        let value = (ui::slider_frac(r, x) * 100.0).round() as i32;
        match t {
            T::ShadowOpacity => {
                self.draft.window.shadow_opacity = value;
                self.preview_shadow();
            }
            T::Blur => {
                self.draft.blur = value;
                self.preview_look();
            }
            T::Tint => {
                self.draft.tint = value;
                self.preview_look();
            }
            T::Transparency => self.draft.item_button.transparency = value,
            _ => {}
        }
    }

    fn column_width(&self) -> i32 {
        self.draft.window.width.round() as i32
    }

    fn set_column_width(&mut self, value: i32) {
        self.draft.window.width = (value as f64).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
    }

    /// 背景画像を設定する。寸法が上限以上・読めない画像は受け付けず、理由を表示する。
    fn set_background(&mut self, path: &str) {
        let name = std::path::Path::new(path)
            .file_name()
            .map_or(path.into(), |n| n.to_string_lossy().into_owned());
        let size = self.gfx.as_ref().and_then(|g| g.image_size(path));
        self.background_message = match size {
            Some((w, h)) if w >= MAX_IMAGE || h >= MAX_IMAGE => Some((
                format!(
                    "画像が大きすぎます ({w}×{h}px)。幅・高さとも {MAX_IMAGE}px 未満の画像を指定してください。({name})"
                ),
                true,
            )),
            Some(_) => {
                self.draft.background_image = path.to_owned();
                self.preview_look();
                None
            }
            None => Some((format!("画像として読み込めません: {name}"), true)),
        };
        self.invalidate();
    }

    fn browse_background(&mut self) {
        // SAFETY: COM のファイル選択ダイアログ。文字列は呼び出し中有効
        let chosen = unsafe {
            let Ok(dlg) =
                CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            else {
                return;
            };
            let filters = [
                COMDLG_FILTERSPEC {
                    pszName: w!("画像ファイル"),
                    pszSpec: w!("*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp;*.tif;*.tiff"),
                },
                COMDLG_FILTERSPEC {
                    pszName: w!("すべてのファイル"),
                    pszSpec: w!("*.*"),
                },
            ];
            let _ = dlg.SetFileTypes(&filters);
            let _ = dlg.SetTitle(w!("背景画像の選択"));
            if dlg.Show(Some(self.hwnd)).is_err() {
                return; // キャンセル
            }
            let Ok(item) = dlg.GetResult() else { return };
            let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) else {
                return;
            };
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        };
        self.set_background(&chosen);
    }

    fn on_drop(&mut self, hdrop: HDROP) {
        // SAFETY: WM_DROPFILES の HDROP を読んで解放する
        let first = unsafe {
            let len = DragQueryFileW(hdrop, 0, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, 0, Some(&mut buf)) as usize;
            DragFinish(hdrop);
            (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
        };
        // 設定画面へのドロップは背景画像の指定だけに使う (本体のアイテム登録とは別ウィンドウ)
        if let Some(path) = first {
            self.set_background(&path);
        }
    }

    fn toggle_modifier(&mut self, i: usize) {
        let h = &mut self.draft.hotkey;
        let flags = [h.ctrl, h.alt, h.shift];
        if i < 3 && flags[i] && flags.iter().filter(|&&f| f).count() == 1 && !h.win {
            self.hotkey_message = Some("修飾キー (Ctrl / Alt / Shift) は1つ以上必要です。".into());
            return;
        }
        match i {
            0 => h.ctrl = !h.ctrl,
            1 => h.alt = !h.alt,
            2 => h.shift = !h.shift,
            _ => {}
        }
        self.hotkey_message = None;
    }

    fn choose_key(&mut self, anchor: Rect) {
        let s = dialog::scale(self.hwnd);
        let mut pt = POINT {
            x: (anchor.x * s) as i32,
            y: ((anchor.y + anchor.h) * s) as i32,
        };
        // SAFETY: メニューは関数内で生成・破棄する
        let cmd = unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let Ok(menu) = CreatePopupMenu() else { return };
            for (i, c) in ('A'..='Z').enumerate() {
                // 9 文字ごとに列を分けて、縦に長くなりすぎないようにする
                let mut flags = MF_STRING;
                if i > 0 && i % 9 == 0 {
                    flags |= MF_MENUBARBREAK;
                }
                if c == self.draft.hotkey.key {
                    flags |= MF_CHECKED;
                }
                let label = crate::platform::wide::to_wide(&c.to_string());
                let _ = AppendMenuW(menu, flags, c as usize, PCWSTR(label.as_ptr()));
            }
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            cmd.0 as u32
        };
        if let Some(c) = char::from_u32(cmd).filter(char::is_ascii_uppercase) {
            self.draft.hotkey.key = c;
        }
    }

    fn open_picker(&mut self, target: ColorTarget, anchor: Rect) {
        let c = match target {
            ColorTarget::Background => self.draft.item_button.background_color,
            ColorTarget::Text => self.draft.item_button.text_color,
        };
        self.picker = Some(Picker {
            target,
            hsv: color::to_hsv(c),
            anchor,
            sv: None,
            hue: None,
        });
    }

    fn set_picked(&mut self, c: Rgb, hsv: Option<Hsv>) {
        let Some(p) = &mut self.picker else { return };
        p.hsv = hsv.unwrap_or_else(|| {
            // 色見本で選んだ無彩色は色相を保つ
            let mut h = color::to_hsv(c);
            if h.s == 0.0 {
                h.h = p.hsv.h;
            }
            h
        });
        match p.target {
            ColorTarget::Background => self.draft.item_button.background_color = c,
            ColorTarget::Text => self.draft.item_button.text_color = c,
        }
    }

    fn drag_picker(&mut self, t: T, x: f32, y: f32) {
        let Some(rects) = self.picker_rects() else {
            return;
        };
        let Some(r) = rects.iter().find(|(c, _)| *c == t).map(|(_, r)| *r) else {
            return;
        };
        let Some(p) = &self.picker else { return };
        let mut hsv = p.hsv;
        match t {
            T::PickSv => {
                hsv.s = ((x - r.x) / r.w).clamp(0.0, 1.0);
                hsv.v = 1.0 - ((y - r.y) / r.h).clamp(0.0, 1.0);
            }
            T::PickHue => hsv.h = ((x - r.x) / r.w).clamp(0.0, 0.9999) * 360.0,
            _ => return,
        }
        self.set_picked(color::to_rgb(hsv), Some(hsv));
    }

    // ───────────── 入力 ─────────────

    fn on_down(&mut self, x: f32, y: f32) {
        let Some(t) = self.hit(x, y) else {
            if self.picker.is_some() {
                self.picker = None; // パネルの外を押したら閉じる
                self.invalidate();
            }
            return;
        };
        if !self.enabled(t) {
            return;
        }
        // SAFETY: マウスキャプチャ
        unsafe { SetCapture(self.hwnd) };
        match t {
            T::ColumnWidth => {
                self.drag = Some(Drag {
                    target: t,
                    start_x: x,
                    start_value: self.column_width(),
                })
            }
            T::ShadowOpacity | T::Transparency | T::Blur | T::Tint => {
                self.drag = Some(Drag {
                    target: t,
                    start_x: x,
                    start_value: 0,
                });
                self.set_slider(t, x);
            }
            T::PickSv | T::PickHue => {
                self.drag = Some(Drag {
                    target: t,
                    start_x: x,
                    start_value: 0,
                });
                self.drag_picker(t, x, y);
            }
            _ => self.pressed = Some(t),
        }
        self.invalidate();
    }

    fn on_move(&mut self, x: f32, y: f32) {
        if let Some(d) = &self.drag {
            let (t, start_x, start_value) = (d.target, d.start_x, d.start_value);
            match t {
                T::ColumnWidth => {
                    // SAFETY: キー状態の参照
                    let fine = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                    // 4 DIP ごとに 10px (Shift なら 1px) 変わる
                    let steps = ((x - start_x) / 4.0).round() as i32;
                    let value = if fine {
                        start_value + steps
                    } else {
                        (start_value + steps * 10) / 10 * 10
                    };
                    self.set_column_width(value);
                }
                T::ShadowOpacity | T::Transparency | T::Blur | T::Tint => self.set_slider(t, x),
                T::PickSv | T::PickHue => self.drag_picker(t, x, y),
                _ => {}
            }
            self.invalidate();
            return;
        }
        if !self.tracking_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: 自分のウィンドウのマウス追跡
            self.tracking_leave = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
        }
        let hit = self.hit(x, y);
        if hit != self.hover {
            self.hover = hit;
            self.invalidate();
        }
    }

    fn on_up(&mut self, x: f32, y: f32) {
        // キャプチャの解放は WM_CAPTURECHANGED を同期で送り、そこで押下状態を消すので、先に取り出す
        let dragged = self.drag.take().is_some();
        let pressed = self.pressed.take();
        // SAFETY: キャプチャ解放
        unsafe {
            let _ = ReleaseCapture();
        }
        if dragged {
            self.invalidate();
            return;
        }
        let Some(t) = pressed else { return };
        if self.hit(x, y) != Some(t) {
            self.invalidate();
            return;
        }
        let anchor = self
            .layout()
            .controls
            .iter()
            .find(|(c, _)| *c == t)
            .map(|(_, r)| self.view(*r));
        match t {
            T::Shadow => {
                self.draft.window.shadow = !self.draft.window.shadow;
                self.preview_shadow();
            }
            T::Bold => self.draft.item_button.bold_text = !self.draft.item_button.bold_text,
            T::Desktop => self.draft.desktop_double_click = !self.draft.desktop_double_click,
            T::Browse | T::DropZone => self.browse_background(),
            T::Clear => {
                self.draft.background_image.clear();
                self.background_message = None;
                self.preview_look();
            }
            T::BackdropKind(i) => {
                self.draft.backdrop = [Backdrop::None, Backdrop::Wallpaper, Backdrop::Acrylic][i];
                self.preview_look();
            }
            T::BgColor => self.open_picker(
                ColorTarget::Background,
                anchor.unwrap_or(Rect::new(x, y, 0.0, 0.0)),
            ),
            T::TextColor => self.open_picker(
                ColorTarget::Text,
                anchor.unwrap_or(Rect::new(x, y, 0.0, 0.0)),
            ),
            T::Reset => self.draft.item_button = ItemButtonSettings::default(),
            T::Modifier(i) => self.toggle_modifier(i),
            T::Key => {
                if let Some(a) = anchor {
                    self.choose_key(a);
                }
            }
            T::Preset(i) => self.set_picked(color::PRESETS[i], None),
            T::PickClose => self.picker = None,
            T::Ok => return self.finish(true),
            T::Cancel => return self.finish(false),
            _ => {}
        }
        self.invalidate();
    }

    fn on_wheel(&mut self, delta: i16, x: f32, y: f32) {
        let notches = delta as f32 / 120.0;
        match self.hit(x, y) {
            Some(T::ColumnWidth) => {
                let v = self.column_width() + (notches.signum() as i32) * 10;
                self.set_column_width(v / 10 * 10);
            }
            Some(t @ (T::ShadowOpacity | T::Transparency | T::Blur | T::Tint))
                if self.enabled(t) =>
            {
                let step = notches.signum() as i32;
                match t {
                    T::Blur | T::Tint => {
                        let v = if t == T::Blur {
                            &mut self.draft.blur
                        } else {
                            &mut self.draft.tint
                        };
                        *v = (*v + step).clamp(0, 100);
                        self.preview_look();
                    }
                    T::ShadowOpacity => {
                        let o = &mut self.draft.window.shadow_opacity;
                        *o = (*o + step).clamp(0, 100);
                        self.preview_shadow();
                    }
                    _ => {
                        let v = &mut self.draft.item_button.transparency;
                        *v = (*v + step).clamp(0, 100);
                    }
                }
            }
            _ if self.picker.is_none() => {
                let layout = self.layout();
                self.scroll = (self.scroll - notches * 60.0).clamp(0.0, self.max_scroll(&layout));
                self.on_move(x, y);
            }
            _ => {}
        }
        self.invalidate();
    }

    fn finish(&mut self, ok: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        if ok {
            RESULT.with(|r| *r.borrow_mut() = Some(self.draft.clone()));
        }
        // SAFETY: 本体への通知とダイアログの破棄
        unsafe {
            let _ = PostMessageW(
                Some(self.owner),
                WM_APP_SETTINGS,
                WPARAM(ok as usize),
                LPARAM(0),
            );
        }
        dialog::close(self.hwnd);
    }

    // ───────────── 描画 ─────────────

    fn paint(&mut self) {
        let Some(gfx) = &mut self.gfx else { return };
        if gfx.ensure_target(self.hwnd).is_err() {
            return;
        }
        self.refresh_bitmaps();
        let layout = self.layout();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(&layout));
        let gfx = self.gfx.as_ref().unwrap();
        gfx.begin(ui::BG);

        gfx.push_clip(self.content_view());
        for (text, r, color, style) in &layout.texts {
            gfx.text(text, self.view(*r), *color, *style);
        }
        for (t, r) in &layout.controls {
            self.draw_control(*t, self.view(*r));
        }
        if layout.height > self.content_view().h {
            let view = self.content_view();
            let bar_h = (view.h * view.h / layout.height).max(30.0);
            let bar_y = (view.h - bar_h) * (self.scroll / self.max_scroll(&layout).max(1.0));
            gfx.fill_round(
                Rect::new(self.width - 6.0, bar_y + 2.0, 3.0, bar_h - 4.0),
                1.5,
                ui::BORDER,
            );
        }
        gfx.pop_clip();

        // 下部の OK / キャンセル (スクロールしない)
        gfx.fill_rect(
            Rect::new(0.0, self.height - FOOTER, self.width, FOOTER),
            ui::PANEL,
        );
        for (t, r) in self.footer_controls() {
            ui::button(
                gfx,
                r,
                if t == T::Ok { "OK" } else { "キャンセル" },
                t == T::Ok,
                self.state(t),
            );
        }

        self.draw_picker();
        if let Some(gfx) = &mut self.gfx {
            gfx.end();
        }
    }

    fn state(&self, t: T) -> State {
        State {
            hover: self.hover == Some(t) && self.drag.is_none(),
            active: self.pressed == Some(t) || self.drag.as_ref().is_some_and(|d| d.target == t),
            disabled: !self.enabled(t),
        }
    }

    fn draw_control(&self, t: T, r: Rect) {
        let gfx = self.gfx.as_ref().unwrap();
        let st = self.state(t);
        let d = &self.draft;
        let rgb = |c: Rgb| Color::rgba(c.0, c.1, c.2, 255);
        match t {
            T::ColumnWidth => ui::scrubber(gfx, r, &format!("{} px", self.column_width()), st),
            T::Shadow => ui::switch(gfx, r, d.window.shadow, st),
            T::Bold => ui::switch(gfx, r, d.item_button.bold_text, st),
            T::Desktop => ui::switch(gfx, r, d.desktop_double_click, st),
            T::ShadowOpacity => ui::slider(gfx, r, d.window.shadow_opacity as f32 / 100.0, st),
            T::Transparency => ui::slider(gfx, r, d.item_button.transparency as f32 / 100.0, st),
            T::Blur => ui::slider(gfx, r, d.blur as f32 / 100.0, st),
            T::Tint => ui::slider(gfx, r, d.tint as f32 / 100.0, st),
            T::BackdropKind(i) => {
                let on = d.backdrop == [Backdrop::None, Backdrop::Wallpaper, Backdrop::Acrylic][i];
                ui::toggle(gfx, r, ["なし", "壁紙ぼかし", "アクリル"][i], on, st);
            }
            T::Browse => ui::button(gfx, r, "参照...", false, st),
            T::Clear => ui::button(gfx, r, "クリア", false, st),
            T::Reset => ui::button(gfx, r, "デフォルトに戻す", false, st),
            T::BgColor => ui::swatch(gfx, r, rgb(d.item_button.background_color), st),
            T::TextColor => ui::swatch(gfx, r, rgb(d.item_button.text_color), st),
            T::Modifier(i) => {
                let h = &d.hotkey;
                let (label, on) = [
                    ("Ctrl", h.ctrl),
                    ("Alt", h.alt),
                    ("Shift", h.shift),
                    ("Win", h.win),
                ][i];
                ui::toggle(gfx, r, label, on, st);
            }
            T::Key => ui::dropdown(gfx, r, &d.hotkey.key.to_string(), st),
            T::DropZone => self.draw_drop_zone(r, st),
            T::Preview => self.draw_preview(r),
            _ => {}
        }
    }

    fn draw_drop_zone(&self, r: Rect, st: State) {
        let gfx = self.gfx.as_ref().unwrap();
        let path = &self.draft.background_image;
        let bitmap = self
            .thumbnail
            .as_ref()
            .and_then(|(_, p, b)| (p == path).then_some(b.as_ref()).flatten());
        gfx.fill_round(r, 8.0, ui::PANEL);
        if let Some(b) = bitmap {
            gfx.push_clip(r.inset(1.0, 1.0));
            gfx.bitmap_cover(b, r);
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
            let bar = Rect::new(r.x, r.y + r.h - 26.0, r.w, 26.0);
            gfx.fill_rect(bar, Color::rgba(0x10, 0x11, 0x14, 0xC0));
            gfx.text(&name, bar.inset(10.0, 0.0), ui::TEXT, TextStyle::Label);
            gfx.pop_clip();
        } else {
            gfx.text(
                "\u{EB9F}",
                Rect::new(r.x, r.y + 22.0, r.w, 30.0),
                ui::MUTED,
                TextStyle::Glyph,
            );
            gfx.text(
                "画像ファイルをここへドロップ (クリックで参照)",
                Rect::new(r.x, r.y + 56.0, r.w, 24.0),
                ui::MUTED,
                TextStyle::Value,
            );
            gfx.text(
                "背景画像なし",
                Rect::new(r.x, r.y + 80.0, r.w, 20.0),
                ui::DISABLED,
                TextStyle::Small,
            );
        }
        gfx.stroke_round(r, 8.0, if st.hover { ui::ACCENT } else { ui::BORDER }, 1.5);
    }

    /// 編集中の外観でアイテムボタンを描く (背景画像があれば下に敷く)。
    fn draw_preview(&self, r: Rect) {
        let gfx = self.gfx.as_ref().unwrap();
        let path = &self.draft.background_image;
        gfx.push_clip(r);
        gfx.fill_round(r, 8.0, Color::rgba(0x22, 0x23, 0x28, 0xFF));
        if let Some(b) = self
            .thumbnail
            .as_ref()
            .and_then(|(_, p, b)| (p == path).then_some(b.as_ref()).flatten())
        {
            gfx.bitmap_cover(b, r);
        }
        let button = Rect::new(
            r.x + 16.0,
            r.y + (r.h - 36.0) / 2.0,
            (r.w - 32.0).min(280.0),
            36.0,
        );
        let hovered = self.hover == Some(T::Preview)
            && button.contains_point(crate::dialog::cursor(self.hwnd));
        ui::item_button(
            gfx,
            button,
            "プレビュー",
            self.icon_bitmap.as_ref().map(|(_, b)| b),
            &self.draft.item_button,
            hovered,
            1.0,
        );
        gfx.pop_clip();
        gfx.stroke_round(r, 8.0, ui::BORDER, 1.0);
    }

    fn draw_picker(&self) {
        let (Some(rects), Some(p), Some(gfx)) = (self.picker_rects(), &self.picker, &self.gfx)
        else {
            return;
        };
        let panel = rects[0].1;
        gfx.fill_round(
            Rect::new(panel.x + 2.0, panel.y + 4.0, panel.w, panel.h),
            10.0,
            Color::rgba(0, 0, 0, 0x60),
        );
        gfx.fill_round(panel, 10.0, ui::PANEL);
        gfx.stroke_round(panel, 10.0, ui::BORDER, 1.0);
        for (t, r) in &rects[1..] {
            match t {
                T::PickSv => {
                    if let Some((_, _, b)) = &p.sv {
                        gfx.bitmap(b, *r, 1.0);
                    }
                    let (cx, cy) = (r.x + p.hsv.s * r.w, r.y + (1.0 - p.hsv.v) * r.h);
                    gfx.stroke_round(
                        Rect::new(cx - 7.0, cy - 7.0, 14.0, 14.0),
                        7.0,
                        Color::rgba(0, 0, 0, 0xA0),
                        3.0,
                    );
                    gfx.stroke_round(
                        Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0),
                        6.0,
                        Color::rgba(255, 255, 255, 0xFF),
                        2.0,
                    );
                }
                T::PickHue => {
                    if let Some((_, b)) = &p.hue {
                        gfx.bitmap(b, *r, 1.0);
                    }
                    let cx = r.x + p.hsv.h / 360.0 * r.w;
                    gfx.stroke_round(
                        Rect::new(cx - 3.0, r.y - 3.0, 6.0, r.h + 6.0),
                        3.0,
                        Color::rgba(255, 255, 255, 0xFF),
                        2.0,
                    );
                }
                T::Preset(i) => {
                    let c = color::PRESETS[*i];
                    ui::swatch(gfx, *r, Color::rgba(c.0, c.1, c.2, 255), self.state(*t));
                }
                T::PickClose => ui::button(gfx, *r, "閉じる", false, self.state(*t)),
                _ => {}
            }
        }
        let c = match p.target {
            ColorTarget::Background => self.draft.item_button.background_color,
            ColorTarget::Text => self.draft.item_button.text_color,
        };
        let close = rects.last().unwrap().1;
        gfx.fill_round(
            Rect::new(panel.x + 12.0, close.y + 4.0, 24.0, 24.0),
            5.0,
            Color::rgba(c.0, c.1, c.2, 255),
        );
        gfx.text(
            &format!("R {}  G {}  B {}", c.0, c.1, c.2),
            Rect::new(panel.x + 44.0, close.y, close.x - panel.x - 50.0, close.h),
            ui::MUTED,
            TextStyle::Label,
        );
    }

    /// レンダーターゲットの世代や内容が変わったビットマップを作り直す。
    fn refresh_bitmaps(&mut self) {
        let Some(gfx) = &self.gfx else { return };
        let generation = gfx.generation;
        if self
            .icon_bitmap
            .as_ref()
            .is_none_or(|(g, _)| *g != generation)
        {
            self.icon_bitmap = self
                .icon
                .as_ref()
                .and_then(|p| gfx.create_bitmap(p))
                .map(|b| (generation, b));
        }
        let path = &self.draft.background_image;
        if self
            .thumbnail
            .as_ref()
            .is_none_or(|(g, p, _)| *g != generation || p != path)
        {
            let bitmap = if path.is_empty() {
                None
            } else {
                gfx.load_image(path)
            };
            self.thumbnail = Some((generation, path.clone(), bitmap));
        }
        if let Some(p) = &mut self.picker {
            let s = 2; // 高 DPI でも滑らかに見えるよう 2 倍の解像度で作る
            if p.sv
                .as_ref()
                .is_none_or(|(g, h, _)| *g != generation || *h != p.hsv.h)
            {
                let (w, h) = ((PICKER_W as usize - 24) * s, SV_H as usize * s);
                let pixels = Pixels {
                    width: w as i32,
                    height: h as i32,
                    data: color::sv_plane(p.hsv.h, w, h),
                };
                p.sv = gfx.create_bitmap(&pixels).map(|b| (generation, p.hsv.h, b));
            }
            if p.hue.as_ref().is_none_or(|(g, _)| *g != generation) {
                let (w, h) = ((PICKER_W as usize - 24) * s, 16 * s);
                let pixels = Pixels {
                    width: w as i32,
                    height: h as i32,
                    data: color::hue_strip(w, h),
                };
                p.hue = gfx.create_bitmap(&pixels).map(|b| (generation, b));
            }
        }
    }
}

impl Dialog for SettingsDialog {
    fn handle(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => {
                self.paint();
                // SAFETY: 描画済みとして領域を確定する
                unsafe {
                    let _ = ValidateRect(Some(hwnd), None);
                }
            }
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SIZE => {
                let (w, h) = (
                    (lparam.0 & 0xFFFF) as u32,
                    ((lparam.0 >> 16) & 0xFFFF) as u32,
                );
                if let Some(g) = &self.gfx {
                    g.resize(w, h);
                }
                let s = dialog::scale(hwnd);
                self.width = w as f32 / s;
                self.height = h as f32 / s;
                self.invalidate();
            }
            WM_DPICHANGED => {
                // SAFETY: lParam は推奨矩形
                let r = unsafe { &*(lparam.0 as *const windows::Win32::Foundation::RECT) };
                if let Some(g) = &self.gfx {
                    g.set_dpi((wparam.0 & 0xFFFF) as u32);
                }
                // SAFETY: 自分のウィンドウの移動
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let (x, y) = dialog::cursor(hwnd);
                let target = self
                    .drag
                    .as_ref()
                    .map(|d| d.target)
                    .or_else(|| self.hit(x, y));
                let cursor = match target {
                    Some(T::ColumnWidth) => IDC_SIZEWE,
                    Some(T::PickPanel) | None => IDC_ARROW,
                    Some(t) if self.enabled(t) => IDC_HAND,
                    _ => IDC_ARROW,
                };
                // SAFETY: システムカーソルの設定
                unsafe {
                    let _ = SetCursor(LoadCursorW(None, cursor).ok());
                }
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let (x, y) = dialog::point(hwnd, lparam);
                self.on_move(x, y);
            }
            WM_MOUSELEAVE => {
                self.tracking_leave = false;
                self.hover = None;
                self.invalidate();
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = dialog::point(hwnd, lparam);
                self.on_down(x, y);
            }
            WM_LBUTTONUP => {
                let (x, y) = dialog::point(hwnd, lparam);
                self.on_up(x, y);
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                self.pressed = None;
                self.invalidate();
            }
            WM_MOUSEWHEEL => {
                let (x, y) = dialog::cursor(hwnd);
                self.on_wheel(((wparam.0 >> 16) & 0xFFFF) as i16, x, y);
            }
            WM_DROPFILES => self.on_drop(HDROP(wparam.0 as *mut _)),
            WM_KEYDOWN => match wparam.0 as u16 {
                k if k == VK_ESCAPE.0 => {
                    if self.picker.take().is_some() {
                        self.invalidate();
                    } else {
                        self.finish(false);
                    }
                }
                k if k == VK_RETURN.0 => self.finish(true),
                _ => return None,
            },
            WM_CLOSE => self.finish(false),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}
