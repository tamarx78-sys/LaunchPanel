//! アイテムの編集ダイアログ。
//!
//! 名前だけは文字の入力欄 (Windows 標準の EDIT。日本語入力・コピー＆ペーストは OS に任せる)。
//! パスは直接入力させず、ファイル・フォルダー・ブラウザーのリンクのドロップ、
//! またはファイル/フォルダーの選択ダイアログで変更する。ボタンの色は色見本から選ぶ。

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DeleteObject, FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS,
    SetBkColor, SetTextColor, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::{BUTTON_COLORS, Item, ItemButtonSettings, Rgb};
use crate::dialog::{self, Dialog};
use crate::droptarget::{self, DROP_DONE, DROP_ENTER, DROP_LEAVE, WM_APP_DROP};
use crate::platform::icon::{self, Pixels};
use crate::platform::wide::to_wide;
use crate::platform::window;
use crate::render::{CachedBitmap, Color, Gfx, Rect, TextStyle};
use crate::ui::{self, State};

/// 編集ダイアログが閉じた (wParam: 保存なら 1)。結果は [`take_result`] で受け取る。
pub const WM_APP_EDIT: u32 = WM_APP + 15;

const MARGIN: f32 = 24.0;
const FOOTER: f32 = 60.0;
const NAME_FRAME: Rect = Rect::new(MARGIN, 46.0, 0.0, 36.0);
const EDIT_H: f32 = 20.0;
const PATH_BOX_Y: f32 = 146.0;
const PATH_BOX_H: f32 = 68.0;
const BUTTONS_Y: f32 = 262.0;
const COLOR_Y: f32 = 346.0;
const SWATCH_W: f32 = 44.0;
const SWATCH_H: f32 = 30.0;
const EDIT_ID: usize = 100;

/// 保存された (アイテム ID, 名前, パス, ボタンの色番号)。
pub struct EditResult {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub color: usize,
}

thread_local! {
    static RESULT: RefCell<Option<EditResult>> = const { RefCell::new(None) };
}

pub fn take_result() -> Option<EditResult> {
    RESULT.with(|r| r.borrow_mut().take())
}

/// 編集ダイアログを開く。`buttons` は色見本に使うボタンの外観。
pub fn open(owner: HWND, id: u64, item: &Item, buttons: &ItemButtonSettings) -> bool {
    let item = item.clone();
    let palette: [Rgb; BUTTON_COLORS] = std::array::from_fn(|i| buttons.color(i));
    dialog::open(
        owner,
        "アイテムの編集",
        480.0,
        COLOR_Y as f64 + SWATCH_H as f64 + 24.0 + 24.0 + FOOTER as f64,
        move |hwnd| Box::new(EditDialog::new(hwnd, owner, id, item, palette)),
    )
    .is_some()
}

#[derive(Clone, Copy, PartialEq)]
enum T {
    File,
    Folder,
    Color(usize),
    Ok,
    Cancel,
}

struct EditDialog {
    hwnd: HWND,
    owner: HWND,
    id: u64,
    path: String,
    color: usize,
    palette: [Rgb; BUTTON_COLORS],
    name_edit: HWND,
    font: HFONT,
    edit_brush: HBRUSH,
    gfx: Option<Gfx>,
    icon: Option<Pixels>,
    icon_bitmap: CachedBitmap,
    width: f32,
    height: f32,
    hover: Option<T>,
    pressed: Option<T>,
    drag_over: bool,
    tracking_leave: bool,
    finished: bool,
}

impl EditDialog {
    fn new(hwnd: HWND, owner: HWND, id: u64, item: Item, palette: [Rgb; BUTTON_COLORS]) -> Self {
        let s = window::scale(hwnd);
        let mut rc = windows::Win32::Foundation::RECT::default();
        // SAFETY: 子ウィンドウ (名前の入力欄) の生成と設定
        let (name_edit, edit_brush) = unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
            let text = to_wide(&item.name);
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(EDIT_ID as *mut _)),
                None,
                None,
            )
            .unwrap_or_default();
            let hint = to_wide("空欄ならパスから自動で付けます");
            // EM_SETCUEBANNER: 空欄の時に薄く表示する案内
            SendMessageW(
                edit,
                0x1501,
                Some(WPARAM(1)),
                Some(LPARAM(hint.as_ptr() as isize)),
            );
            // Enter (保存) と Esc (キャンセル) を入力欄からも受け付ける
            let _ = SetWindowSubclass(edit, Some(edit_keys), 1, hwnd.0 as usize);
            let brush = CreateSolidBrush(colorref(ui::CONTROL));
            (edit, brush)
        };
        droptarget::register(hwnd);
        let mut dialog = Self {
            hwnd,
            owner,
            id,
            path: item.path,
            color: item.color.min(BUTTON_COLORS - 1),
            palette,
            name_edit,
            font: HFONT::default(),
            edit_brush,
            gfx: Gfx::new().ok(),
            icon: None,
            icon_bitmap: None,
            width: (rc.right - rc.left) as f32 / s,
            height: (rc.bottom - rc.top) as f32 / s,
            hover: None,
            pressed: None,
            drag_over: false,
            tracking_leave: false,
            finished: false,
        };
        dialog.update_font();
        dialog.place_edit();
        dialog.load_icon();
        // SAFETY: 入力欄へフォーカスを移し、全選択して書き換えやすくする
        unsafe {
            let _ = SetFocus(Some(name_edit));
            SendMessageW(
                name_edit,
                0xB1, /* EM_SETSEL */
                Some(WPARAM(0)),
                Some(LPARAM(-1)),
            );
        }
        dialog
    }

    fn invalidate(&self) {
        // SAFETY: 再描画要求
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn content_width(&self) -> f32 {
        (self.width - MARGIN * 2.0).max(100.0)
    }

    fn name_frame(&self) -> Rect {
        Rect::new(
            NAME_FRAME.x,
            NAME_FRAME.y,
            self.content_width(),
            NAME_FRAME.h,
        )
    }

    fn path_box(&self) -> Rect {
        Rect::new(MARGIN, PATH_BOX_Y, self.content_width(), PATH_BOX_H)
    }

    fn controls(&self) -> Vec<(T, Rect)> {
        let y = self.height - FOOTER + (FOOTER - 32.0) / 2.0;
        let mut v = vec![
            (T::File, Rect::new(MARGIN, BUTTONS_Y, 120.0, 30.0)),
            (T::Folder, Rect::new(MARGIN + 128.0, BUTTONS_Y, 120.0, 30.0)),
        ];
        for i in 0..BUTTON_COLORS {
            v.push((
                T::Color(i),
                Rect::new(
                    MARGIN + i as f32 * (SWATCH_W + 10.0),
                    COLOR_Y,
                    SWATCH_W,
                    SWATCH_H,
                ),
            ));
        }
        v.push((
            T::Ok,
            Rect::new(self.width - MARGIN - 108.0 * 2.0 + 8.0, y, 100.0, 32.0),
        ));
        v.push((
            T::Cancel,
            Rect::new(self.width - MARGIN - 100.0, y, 100.0, 32.0),
        ));
        v
    }

    fn hit(&self, x: f32, y: f32) -> Option<T> {
        self.controls()
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(t, _)| *t)
    }

    /// DPI に合わせた入力欄のフォント。
    fn update_font(&mut self) {
        let s = window::scale(self.hwnd);
        // SAFETY: フォントの生成と差し替え (古いものは解放する)
        unsafe {
            let font = CreateFontW(
                -(13.0 * s).round() as i32,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            SendMessageW(
                self.name_edit,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            if !self.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
            self.font = font;
        }
    }

    /// 入力欄を枠の内側 (縦中央) に置く。枠は Direct2D で描く。
    fn place_edit(&self) {
        let s = window::scale(self.hwnd);
        let f = self.name_frame();
        let (x, y, w, h) = (f.x + 10.0, f.y + (f.h - EDIT_H) / 2.0, f.w - 20.0, EDIT_H);
        // SAFETY: 子ウィンドウの移動
        unsafe {
            let _ = SetWindowPos(
                self.name_edit,
                None,
                (x * s) as i32,
                (y * s) as i32,
                (w * s) as i32,
                (h * s) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn load_icon(&mut self) {
        let px = (32.0 * window::scale(self.hwnd)).ceil() as i32;
        self.icon = icon::load(&self.path, px);
        self.icon_bitmap = None;
        self.invalidate();
    }

    fn set_path(&mut self, path: String) {
        if path.trim().is_empty() {
            return;
        }
        self.path = path.trim().to_owned();
        self.load_icon();
    }

    fn browse(&mut self, folder: bool) {
        let title = if folder {
            "フォルダーの選択"
        } else {
            "ファイルの選択"
        };
        if let Some(path) = window::pick_file(self.hwnd, title, &[], folder) {
            self.set_path(path);
        }
    }

    fn name_text(&self) -> String {
        // SAFETY: 入力欄の文字列の取得
        unsafe {
            let len = GetWindowTextLengthW(self.name_edit) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = GetWindowTextW(self.name_edit, &mut buf) as usize;
            String::from_utf16_lossy(&buf[..n])
        }
    }

    fn finish(&mut self, ok: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        if ok {
            // 名前が空欄なら、パスから自動で付け直す
            let name = self.name_text().trim().to_owned();
            let name = if name.is_empty() {
                crate::items::default_name(&self.path)
            } else {
                name
            };
            RESULT.with(|r| {
                *r.borrow_mut() = Some(EditResult {
                    id: self.id,
                    name,
                    path: self.path.clone(),
                    color: self.color,
                })
            });
        }
        droptarget::revoke(self.hwnd);
        // SAFETY: 本体への通知とダイアログの破棄
        unsafe {
            let _ = PostMessageW(
                Some(self.owner),
                WM_APP_EDIT,
                WPARAM(ok as usize),
                LPARAM(0),
            );
        }
        dialog::close(self.hwnd);
    }

    fn paint(&mut self) {
        let Some(gfx) = &mut self.gfx else { return };
        if gfx.ensure_target(self.hwnd).is_err() {
            return;
        }
        gfx.refresh_bitmap(&mut self.icon_bitmap, self.icon.as_ref());
        let gfx = self.gfx.as_ref().unwrap();
        let cw = self.content_width();
        gfx.begin(ui::BG);

        gfx.text(
            "名前",
            Rect::new(MARGIN, 16.0, cw, 26.0),
            ui::TEXT,
            TextStyle::Label,
        );
        let frame = self.name_frame();
        // SAFETY: フォーカスの参照
        let focused = unsafe { GetFocus() } == self.name_edit;
        gfx.fill_round(frame, 6.0, ui::CONTROL);
        gfx.stroke_round(
            frame,
            6.0,
            if focused { ui::ACCENT } else { ui::BORDER },
            if focused { 1.5 } else { 1.0 },
        );
        gfx.text(
            "空欄にすると、パスから自動で名前を付けます。",
            Rect::new(MARGIN, frame.y + frame.h + 6.0, cw, 20.0),
            ui::MUTED,
            TextStyle::Caption,
        );

        gfx.text(
            "パスまたは URL",
            Rect::new(MARGIN, PATH_BOX_Y - 30.0, cw, 26.0),
            ui::TEXT,
            TextStyle::Label,
        );
        let pb = self.path_box();
        gfx.fill_round(pb, 8.0, ui::PANEL);
        gfx.stroke_round(
            pb,
            8.0,
            if self.drag_over {
                ui::ACCENT
            } else {
                ui::BORDER
            },
            if self.drag_over { 2.0 } else { 1.0 },
        );
        if let Some((_, b)) = &self.icon_bitmap {
            gfx.bitmap(
                b,
                Rect::new(pb.x + 14.0, pb.y + (pb.h - 32.0) / 2.0, 32.0, 32.0),
                1.0,
            );
        }
        let text_r = Rect::new(pb.x + 58.0, pb.y + 10.0, pb.w - 70.0, pb.h - 20.0);
        let (_, th) = gfx.measure(&self.path, TextStyle::Caption, text_r.w);
        let text_r = Rect::new(
            text_r.x,
            pb.y + ((pb.h - th) / 2.0).max(8.0),
            text_r.w,
            pb.h - 16.0,
        );
        gfx.push_clip(pb.inset(2.0, 6.0));
        gfx.text(&self.path, text_r, ui::TEXT, TextStyle::Caption);
        gfx.pop_clip();
        gfx.text(
            "ファイル・フォルダー・ブラウザーのリンクをこの画面へドロップするか、選択して変更します。",
            Rect::new(MARGIN, pb.y + pb.h + 8.0, cw, 36.0),
            ui::MUTED,
            TextStyle::Caption,
        );

        gfx.text(
            "ボタンの色",
            Rect::new(MARGIN, COLOR_Y - 30.0, cw, 26.0),
            ui::TEXT,
            TextStyle::Label,
        );
        gfx.text(
            &format!(
                "{} (それぞれの色は設定画面で変えられます)",
                crate::app::color_label(self.color)
            ),
            Rect::new(MARGIN, COLOR_Y + SWATCH_H + 8.0, cw, 20.0),
            ui::MUTED,
            TextStyle::Caption,
        );

        gfx.fill_rect(
            Rect::new(0.0, self.height - FOOTER, self.width, FOOTER),
            ui::PANEL,
        );
        for (t, r) in self.controls() {
            let st = State {
                hover: self.hover == Some(t),
                active: self.pressed == Some(t),
                disabled: false,
            };
            let label = match t {
                T::File => "ファイル...",
                T::Folder => "フォルダー...",
                T::Color(i) => {
                    let c = self.palette[i];
                    ui::swatch(gfx, r, Color::rgba(c.0, c.1, c.2, 255), st);
                    if i == self.color {
                        // 選んでいる色は外側の輪と印で示す
                        gfx.stroke_round(r.inset(-3.0, -3.0), 8.0, ui::ACCENT, 2.0);
                        let light =
                            c.0 as u32 * 299 + c.1 as u32 * 587 + c.2 as u32 * 114 > 150_000;
                        let mark = if light { ui::BG } else { ui::TEXT };
                        gfx.text("\u{E73E}", r, mark, TextStyle::Glyph);
                    }
                    continue;
                }
                T::Ok => "保存",
                T::Cancel => "キャンセル",
            };
            ui::button(gfx, r, label, t == T::Ok, st);
        }
        if let Some(g) = &mut self.gfx {
            g.end();
        }
    }
}

impl Dialog for EditDialog {
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
                let (w, h) = window::client_size(lparam);
                if let Some(g) = &self.gfx {
                    g.resize(w, h);
                }
                let s = window::scale(hwnd);
                self.width = w as f32 / s;
                self.height = h as f32 / s;
                self.place_edit();
                self.invalidate();
            }
            WM_DPICHANGED => {
                if let Some(g) = &self.gfx {
                    g.set_dpi((wparam.0 & 0xFFFF) as u32);
                }
                self.update_font();
                // SAFETY: WM_DPICHANGED の lParam
                unsafe { window::apply_suggested_rect(hwnd, lparam) };
            }
            WM_CTLCOLOREDIT => {
                // 入力欄を暗い配色にする
                let hdc = HDC(wparam.0 as *mut _);
                // SAFETY: 渡された DC への色設定
                unsafe {
                    SetTextColor(hdc, colorref(ui::TEXT));
                    SetBkColor(hdc, colorref(ui::CONTROL));
                }
                return Some(LRESULT(self.edit_brush.0 as isize));
            }
            WM_COMMAND if (wparam.0 & 0xFFFF) == EDIT_ID => {
                // フォーカスの出入りで枠の色を変える
                self.invalidate();
                return None;
            }
            WM_SETCURSOR
                if (lparam.0 & 0xFFFF) as u32 == HTCLIENT
                    && wparam.0 as isize == hwnd.0 as isize =>
            {
                let (x, y) = window::cursor(hwnd);
                let cursor = if self.hit(x, y).is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                window::set_cursor(cursor);
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let (x, y) = window::point(hwnd, lparam);
                if !self.tracking_leave {
                    self.tracking_leave = window::track_leave(hwnd);
                }
                let hit = self.hit(x, y);
                if hit != self.hover {
                    self.hover = hit;
                    self.invalidate();
                }
            }
            WM_MOUSELEAVE => {
                self.tracking_leave = false;
                self.hover = None;
                self.invalidate();
            }
            WM_LBUTTONDOWN => {
                let (x, y) = window::point(hwnd, lparam);
                self.pressed = self.hit(x, y);
                if self.pressed.is_some() {
                    // SAFETY: マウスキャプチャ
                    unsafe { SetCapture(hwnd) };
                }
                self.invalidate();
            }
            WM_LBUTTONUP => {
                let (x, y) = window::point(hwnd, lparam);
                // キャプチャの解放は WM_CAPTURECHANGED を同期で送るので、先に押下状態を取り出す
                let pressed = self.pressed.take();
                // SAFETY: キャプチャ解放
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.invalidate();
                if let Some(t) = pressed.filter(|&t| self.hit(x, y) == Some(t)) {
                    match t {
                        T::File => self.browse(false),
                        T::Folder => self.browse(true),
                        T::Color(i) => self.color = i,
                        T::Ok => self.finish(true),
                        T::Cancel => self.finish(false),
                    }
                }
            }
            WM_CAPTURECHANGED => {
                self.pressed = None;
                self.invalidate();
            }
            WM_APP_DROP => {
                match wparam.0 {
                    DROP_ENTER => self.drag_over = true,
                    DROP_LEAVE => self.drag_over = false,
                    DROP_DONE => {
                        // SAFETY: DROP_DONE の lParam を 1 回だけ取り出す
                        let path = unsafe { droptarget::take_dropped(lparam) };
                        self.set_path(path);
                    }
                    _ => {}
                }
                self.invalidate();
            }
            WM_KEYDOWN => match wparam.0 as u16 {
                k if k == VK_ESCAPE.0 => self.finish(false),
                k if k == VK_RETURN.0 => self.finish(true),
                _ => return None,
            },
            WM_CLOSE => self.finish(false),
            WM_DESTROY => {
                // SAFETY: このダイアログで作った GDI 資源の解放
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(self.font.0));
                    let _ = DeleteObject(HGDIOBJ(self.edit_brush.0));
                }
                return None;
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

fn colorref(c: Color) -> COLORREF {
    let to = |v: f32| (v * 255.0).round() as u32;
    COLORREF(to(c.0) | (to(c.1) << 8) | (to(c.2) << 16))
}

/// 名前の入力欄で押された Enter / Esc を、ダイアログの保存・キャンセルとして親へ回す。
unsafe extern "system" fn edit_keys(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    parent: usize,
) -> LRESULT {
    let key = wparam.0 as u16;
    let is_action = key == VK_RETURN.0 || key == VK_ESCAPE.0;
    // SAFETY: 親ウィンドウへの転送と既定処理
    unsafe {
        match msg {
            WM_KEYDOWN if is_action => {
                let _ = PostMessageW(Some(HWND(parent as *mut _)), WM_KEYDOWN, wparam, LPARAM(0));
                LRESULT(0)
            }
            // 対応する WM_CHAR ('\r' と Esc) は捨てて、警告音を鳴らさない
            WM_CHAR if wparam.0 == 0x0D || wparam.0 == 0x1B => LRESULT(0),
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}
