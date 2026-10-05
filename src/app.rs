//! メインウィンドウ。状態・レイアウト・描画・入力処理・アニメーション。
//!
//! 座標は特記しない限りクライアント領域の DIP。アイテムの位置は「コンテンツ座標」
//! (アイテム領域の左上原点、スクロール前) で持ち、描画時にスクロール量を引く。
//! アニメーションは描画のたびに目標値へ指数的に近づけ、動いている間だけ次のフレームを要求する。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    ValidateRect,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::{Config, Item, MIN_INNER_SIZE, Settings, Store};
use crate::platform::icon::Pixels;
use crate::platform::wide::to_wide;
use crate::platform::{desktop, foreground, shadow, shell};
use crate::render::{Color, Gfx, Rect, TextStyle};
use crate::services::IconLoader;
use crate::{items, layout};

// ───────────── 定数 ─────────────

pub const WM_APP_SHOW: u32 = WM_APP + 10;
pub const WM_APP_SHELL: u32 = WM_APP + 11;
pub const WM_APP_ICON: u32 = WM_APP + 12;
pub const WM_APP_DESKTOP: u32 = WM_APP + 13;

const TIMER_GRACE: usize = 1;
const TIMER_FOOTER: usize = 2;

const TOP_BAR: f32 = 40.0;
const FOOTER: f32 = 40.0;
const PAD: f32 = 6.0;
const BUTTON_H: f32 = 36.0;
const ROW_H: f32 = BUTTON_H + 4.0;
const ICON: f32 = 20.0;
const EDGE: f32 = 6.0;
const CORNER: f32 = 28.0;
const DRAG_THRESHOLD: f32 = 6.0;
const AUTO_SCROLL_MARGIN: f32 = 24.0;
/// 復帰直後のフォーカス遷移を「フォーカス喪失」と誤判定しないための猶予
const RESTORE_GRACE: Duration = Duration::from_millis(400);
/// 位置とスクロールのアニメーションの時定数 (秒)。約 4 倍の時間でほぼ収束する
const MOVE_TAU: f32 = 0.045;
const SCROLL_TAU: f32 = 0.06;
const FALLBACK_ICON_ID: u64 = 0;
const FOOTER_HINT: &str = "ドラッグ＆ドロップで追加できます";

const BG: Color = Color::rgba(0x22, 0x23, 0x28, 0xFF);
const PILL: Color = Color::rgba(0x14, 0x15, 0x18, 0xB0);
const PILL_HOVER: Color = Color::rgba(0x3A, 0x3C, 0x44, 0xE0);
const OVERLAY_TEXT: Color = Color::rgba(0xE8, 0xE8, 0xEC, 0xFF);
const HOVER_OUTLINE: Color = Color::rgba(0x6C, 0xB4, 0xFF, 0xFF);
const HOVER_FILL: Color = Color::rgba(0xFF, 0xFF, 0xFF, 0x24);

const DIR_LEFT: u8 = 1;
const DIR_TOP: u8 = 2;
const DIR_RIGHT: u8 = 4;
const DIR_BOTTOM: u8 = 8;

// ───────────── 状態 ─────────────

struct Entry {
    id: u64,
    item: Item,
    icon: Option<Pixels>,
    icon_failed: bool,
    bitmap: Option<(u64, ID2D1Bitmap)>,
    /// 現在の描画位置と目標位置 (コンテンツ座標)
    pos: (f32, f32),
    target: (f32, f32),
    width: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Resize(u8),
    Caption,
    Gear,
    Pin,
    Item(u64),
}

struct Press {
    id: u64,
    start: (f32, f32),
    /// ボタン内で掴んだ位置
    grab: (f32, f32),
}

struct Drag {
    id: u64,
    order: Vec<u64>,
    pointer: (f32, f32),
    grab: (f32, f32),
}

struct ResizeOp {
    dir: u8,
    cursor: POINT,
    rect: RECT,
}

pub struct App {
    hwnd: HWND,
    store: Store,
    settings: Settings,
    entries: Vec<Entry>,
    next_id: u64,
    gfx: Gfx,
    icons: IconLoader,
    fallback: Option<Pixels>,
    fallback_bitmap: Option<(u64, ID2D1Bitmap)>,
    background: Option<(u64, ID2D1Bitmap)>,
    background_tried: u64,

    width: f32,
    height: f32,
    slots: Vec<(u64, Rect)>,
    content_height: f32,
    scroll: f32,
    scroll_target: f32,
    animate_moves: bool,
    last_frame: Instant,

    hover: Hit,
    tracking_leave: bool,
    press: Option<Press>,
    drag: Option<Drag>,
    resize: Option<ResizeOp>,

    pinned: bool,
    modal: u32,
    shown_at: Instant,
    /// 復帰直後の猶予中に前面を奪われた。猶予の終わりに取り戻す
    reclaim_focus: bool,
    exiting: bool,
    footer: Option<String>,
}

impl App {
    pub fn new(hwnd: HWND, store: Store, config: Config) -> windows::core::Result<Box<Self>> {
        let gfx = Gfx::new()?;
        // SAFETY: 有効なウィンドウ
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let icon_px = ((ICON * dpi as f32 / 96.0).ceil() as i32).max(32);
        let icons = IconLoader::new(hwnd, WM_APP_ICON, icon_px);
        // 取得失敗時のアイコンは LaunchPanel 自身のアイコン
        if let Ok(exe) = std::env::current_exe() {
            icons.request(FALLBACK_ICON_ID, &exe.to_string_lossy());
        }
        let mut app = Box::new(Self {
            hwnd,
            store,
            settings: config.settings,
            entries: Vec::new(),
            next_id: FALLBACK_ICON_ID + 1,
            gfx,
            icons,
            fallback: None,
            fallback_bitmap: None,
            background: None,
            background_tried: 0,
            width: 0.0,
            height: 0.0,
            slots: Vec::new(),
            content_height: 0.0,
            scroll: 0.0,
            scroll_target: 0.0,
            animate_moves: false,
            last_frame: Instant::now(),
            hover: Hit::None,
            tracking_leave: false,
            press: None,
            drag: None,
            resize: None,
            pinned: false,
            modal: 0,
            shown_at: Instant::now(),
            reclaim_focus: false,
            exiting: false,
            footer: None,
        });
        for item in config.items {
            app.push_entry(item);
        }
        Ok(app)
    }

    fn push_entry(&mut self, item: Item) {
        let id = self.next_id;
        self.next_id += 1;
        self.icons.request(id, &item.path);
        self.entries.push(Entry {
            id,
            item,
            icon: None,
            icon_failed: false,
            bitmap: None,
            pos: (0.0, 0.0),
            target: (0.0, 0.0),
            width: 0.0,
        });
    }

    fn scale(&self) -> f32 {
        // SAFETY: 有効なウィンドウ
        unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32 / 96.0
    }

    // ───────────── 起動・表示 ─────────────

    /// 枠なしの外観・影・初期サイズ・トレイなどを整える。
    pub fn start(&mut self) {
        let hwnd = self.hwnd;
        // SAFETY: 自分のウィンドウへの設定
        unsafe {
            let corner = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                size_of_val(&corner) as u32,
            );
            windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true);
        }
        self.place_initially();
        self.apply_shadow();

        let h = &self.settings.hotkey;
        let modifiers = (h.ctrl as u32 * 2) | (h.alt as u32) | (h.shift as u32 * 4) | (h.win as u32 * 8);
        let (tip, show, settings, exit) = (to_wide("LaunchPanel3"), to_wide("表示"), to_wide("設定"), to_wide("終了"));
        let icon = to_wide("");
        // SAFETY: 文字列は呼び出し中有効。コールバックはプロセス終了まで有効
        let status = unsafe {
            shell::lp_shell_start(
                on_shell_event,
                tip.as_ptr(),
                show.as_ptr(),
                settings.as_ptr(),
                exit.as_ptr(),
                icon.as_ptr(),
                modifiers,
                h.key as u32,
            )
        };
        if status & 2 == 0 {
            crate::log::write(&format!("ホットキーの登録に失敗しました: {modifiers:#x}+{} (他のアプリが使用中の可能性があります)", h.key));
        }
        if self.settings.desktop_double_click && desktop::lp_desktop_start(on_desktop_double_click) == 0 {
            crate::log::write("デスクトップのダブルクリック監視を開始できませんでした。");
        }
        self.show();
    }

    /// 保存済みの内側サイズで、プライマリモニターの作業領域中央に置く。
    fn place_initially(&mut self) {
        let scale = self.scale() as f64;
        let w = (self.settings.window.effective_inner_width() * scale).round() as i32;
        let h = (self.settings.window.effective_inner_height() * scale).round() as i32;
        let area = work_area(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let x = area.left + ((area.right - area.left - w) / 2).max(0);
        let y = area.top + ((area.bottom - area.top - h) / 2).max(0);
        // SAFETY: 自分のウィンドウの移動
        let _ = unsafe { SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    fn apply_shadow(&self) {
        let w = &self.settings.window;
        // SAFETY: 自分のウィンドウへの影の設定
        unsafe { shadow::lp_set_window_shadow(self.hwnd.0 as isize, w.shadow as i32, w.shadow_opacity) };
    }

    /// 従来の位置で復帰し、前面化して入力フォーカスを得る。
    pub fn show(&mut self) {
        self.shown_at = Instant::now();
        self.reclaim_focus = false;
        // SAFETY: 自分のウィンドウの表示
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let ok = foreground::lp_force_foreground(self.hwnd.0 as isize) != 0;
            crate::log::debug(&format!("表示: 前面化{} (前面={})", if ok { "成功" } else { "失敗" }, window_label(GetForegroundWindow())));
        }
        self.invalidate();
    }

    /// デスクトップのダブルクリック位置 (物理座標) へ左上を合わせて復帰する。
    /// 非表示のまま移動してから表示するので、以前の位置は見えない。
    fn show_at(&mut self, x: i32, y: i32) {
        crate::log::debug(&format!("デスクトップのダブルクリックで復帰: ({x}, {y})"));
        let mut rc = RECT::default();
        // SAFETY: 自分のウィンドウの矩形取得と移動
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rc);
            let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
            let area = work_area(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let x = x.min(area.right - w).max(area.left);
            let y = y.min(area.bottom - h).max(area.top);
            let _ = SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        self.show();
    }

    fn hide(&self) {
        // SAFETY: 前面ウィンドウの参照
        crate::log::debug(&format!("非表示 (前面={})", window_label(unsafe { GetForegroundWindow() })));
        // SAFETY: 自分のウィンドウの非表示
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// ピン留め中・ダイアログ表示中でなく、前面でもなければ非表示にする。
    fn auto_hide(&mut self) {
        // SAFETY: ウィンドウ状態の参照
        let visible = unsafe { IsWindowVisible(self.hwnd) }.as_bool();
        if self.pinned || self.exiting || self.modal > 0 || !visible || self.resize.is_some() {
            return;
        }
        let elapsed = self.shown_at.elapsed();
        if elapsed < RESTORE_GRACE {
            // 復帰直後のフォーカス遷移 (デスクトップのダブルクリックを Explorer が後から処理して
            // 前面を取り返す等) はフォーカス喪失と見なさない。猶予の終わりに前面を取り戻す
            crate::log::debug(&format!("復帰直後に非アクティブ化 ({} ms)。猶予後に前面を取り戻す", elapsed.as_millis()));
            self.reclaim_focus = true;
            // SAFETY: タイマー設定
            unsafe { SetTimer(Some(self.hwnd), TIMER_GRACE, (RESTORE_GRACE - elapsed).as_millis() as u32 + 1, None) };
            return;
        }
        if std::mem::take(&mut self.reclaim_focus) {
            // SAFETY: 前面ウィンドウの参照と前面化
            unsafe {
                if GetForegroundWindow() != self.hwnd {
                    let ok = foreground::lp_force_foreground(self.hwnd.0 as isize) != 0;
                    crate::log::debug(&format!("猶予終了: 前面を取り戻す ({})", if ok { "成功" } else { "失敗" }));
                }
            }
            return;
        }
        // SAFETY: 前面ウィンドウの参照
        if unsafe { GetForegroundWindow() } == self.hwnd {
            return;
        }
        self.hide();
    }

    fn exit(&mut self) {
        self.exiting = true;
        desktop::lp_desktop_stop();
        shell::lp_shell_stop();
        // SAFETY: 自分のウィンドウの破棄
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    // ───────────── レイアウト ─────────────

    fn invalidate(&self) {
        // SAFETY: 再描画要求
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn item_area(&self) -> Rect {
        Rect::new(0.0, TOP_BAR, self.width, (self.height - TOP_BAR - FOOTER).max(0.0))
    }

    fn column_count(&self) -> usize {
        layout::column_count(self.width as f64, self.settings.window.width, self.entries.len())
    }

    /// 現在の幅から列数を決め、左列の上から下へ、次に右の列へ配置する。
    /// `animate` が false なら目標位置へ即座に移す (リサイズなど)。
    fn relayout(&mut self, animate: bool) {
        if self.width <= 0.0 {
            return;
        }
        let order: Vec<u64> = match &self.drag {
            Some(d) => d.order.clone(),
            None => self.entries.iter().map(|e| e.id).collect(),
        };
        let columns = self.column_count();
        let column_w = self.width / columns as f32;
        let cells = layout::cells(order.len(), columns);
        self.slots.clear();
        let mut rows = 0;
        for (&id, &(c, r)) in order.iter().zip(&cells) {
            let target = (c as f32 * column_w + PAD, r as f32 * ROW_H);
            if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                e.target = target;
                e.width = (column_w - PAD * 2.0).max(0.0);
                if !animate {
                    e.pos = target;
                }
            }
            self.slots.push((id, Rect::new(c as f32 * column_w, r as f32 * ROW_H, column_w, ROW_H)));
            rows = rows.max(r + 1);
        }
        self.content_height = rows as f32 * ROW_H;
        self.scroll_target = self.scroll_target.clamp(0.0, self.max_scroll());
        if !animate {
            self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        }
        self.animate_moves = animate;
        self.invalidate();
    }

    fn max_scroll(&self) -> f32 {
        (self.content_height - self.item_area().h).max(0.0)
    }

    // ───────────── ヒットテスト ─────────────

    fn gear_rect(&self) -> Rect {
        Rect::new(self.width - 8.0 - 30.0 - 4.0 - 30.0, 6.0, 30.0, 28.0)
    }

    fn pin_rect(&self) -> Rect {
        Rect::new(self.width - 8.0 - 30.0, 6.0, 30.0, 28.0)
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        let (w, h) = (self.width, self.height);
        let (on_left, on_right, on_top, on_bottom) = (x < EDGE, x >= w - EDGE, y < EDGE, y >= h - EDGE);
        if on_left || on_right || on_top || on_bottom {
            // 端の帯の上では、角から CORNER 以内を斜め方向として扱う (角は辺より広く掴める)
            let (near_left, near_right, near_top, near_bottom) = (x < CORNER, x >= w - CORNER, y < CORNER, y >= h - CORNER);
            let horizontal_edge = on_top || on_bottom;
            let vertical_edge = on_left || on_right;
            let mut dir = 0;
            if on_left || horizontal_edge && near_left {
                dir |= DIR_LEFT;
            }
            if on_right || horizontal_edge && near_right {
                dir |= DIR_RIGHT;
            }
            if on_top || vertical_edge && near_top {
                dir |= DIR_TOP;
            }
            if on_bottom || vertical_edge && near_bottom {
                dir |= DIR_BOTTOM;
            }
            return Hit::Resize(dir);
        }
        if y < TOP_BAR {
            if self.gear_rect().contains(x, y) {
                return Hit::Gear;
            }
            if self.pin_rect().contains(x, y) {
                return Hit::Pin;
            }
            return Hit::Caption;
        }
        if self.item_area().contains(x, y) {
            let (cx, cy) = (x, y - TOP_BAR + self.scroll);
            for e in &self.entries {
                if Rect::new(e.target.0, e.target.1, e.width, BUTTON_H).contains(cx, cy) {
                    return Hit::Item(e.id);
                }
            }
        }
        Hit::None
    }

    fn entry(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    // ───────────── 入力 ─────────────

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        if self.resize.is_some() {
            self.update_resize();
            return;
        }
        if let Some(p) = &self.press {
            if self.drag.is_none()
                && ((x - p.start.0).abs() >= DRAG_THRESHOLD || (y - p.start.1).abs() >= DRAG_THRESHOLD)
            {
                self.drag = Some(Drag {
                    id: p.id,
                    order: self.entries.iter().map(|e| e.id).collect(),
                    pointer: (x, y),
                    grab: p.grab,
                });
                self.hover = Hit::None;
            }
        }
        if self.drag.is_some() {
            self.update_drag(x, y);
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
        let hit = match self.hit(x, y) {
            h @ (Hit::Item(_) | Hit::Gear | Hit::Pin) => h,
            _ => Hit::None,
        };
        if hit != self.hover {
            self.hover = hit;
            self.invalidate();
        }
    }

    fn on_left_down(&mut self, x: f32, y: f32) {
        match self.hit(x, y) {
            Hit::Resize(dir) => {
                let mut cursor = POINT::default();
                let mut rect = RECT::default();
                // SAFETY: カーソル位置と自分のウィンドウ矩形の取得、マウスキャプチャ
                unsafe {
                    let _ = GetCursorPos(&mut cursor);
                    let _ = GetWindowRect(self.hwnd, &mut rect);
                    SetCapture(self.hwnd);
                }
                self.resize = Some(ResizeOp { dir, cursor, rect });
            }
            Hit::Caption => {
                // システムの移動ループに任せる (滑らかで、モニターをまたいでも DPI が正しく扱われる)
                // SAFETY: 自分のウィンドウへのメッセージ送信
                unsafe {
                    let _ = ReleaseCapture();
                    SendMessageW(self.hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(LPARAM(0)));
                }
            }
            Hit::Item(id) => {
                let Some(e) = self.entry(id) else { return };
                let grab = (x - e.pos.0, y - (TOP_BAR + e.pos.1 - self.scroll));
                self.press = Some(Press { id, start: (x, y), grab });
                // SAFETY: マウスキャプチャ
                unsafe { SetCapture(self.hwnd) };
            }
            _ => {}
        }
    }

    fn on_left_up(&mut self, x: f32, y: f32) {
        if let Some(op) = self.resize.take() {
            // SAFETY: キャプチャ解放
            unsafe {
                let _ = ReleaseCapture();
            }
            self.finish_resize(op.dir);
            return;
        }
        if self.drag.is_some() {
            self.finish_drag();
            // SAFETY: キャプチャ解放
            unsafe {
                let _ = ReleaseCapture();
            }
            return;
        }
        if let Some(p) = self.press.take() {
            // SAFETY: キャプチャ解放
            unsafe {
                let _ = ReleaseCapture();
            }
            if self.hit(x, y) == Hit::Item(p.id) {
                if let Some(e) = self.entry(p.id) {
                    let path = e.item.path.clone();
                    let name = e.item.name.clone();
                    if !crate::services::launch(self.hwnd, &path) {
                        self.show_footer(&format!("起動できません: {name}"));
                    }
                }
            }
            return;
        }
        match self.hit(x, y) {
            Hit::Pin => {
                self.pinned = !self.pinned;
                self.invalidate();
            }
            Hit::Gear => self.show_footer("設定画面は次の段階で実装予定です"),
            _ => {}
        }
    }

    /// キャプチャを失った (別ウィンドウの割込みなど) 場合は、操作をその時点で終える。
    fn on_capture_lost(&mut self) {
        if let Some(op) = self.resize.take() {
            self.finish_resize(op.dir);
        }
        if self.drag.is_some() {
            self.finish_drag();
        }
        self.press = None;
    }

    fn on_wheel(&mut self, delta: i16) {
        self.scroll_target = (self.scroll_target - delta as f32 / 120.0 * ROW_H * 1.5).clamp(0.0, self.max_scroll());
        self.invalidate();
    }

    fn on_right_up(&mut self, x: f32, y: f32) {
        let Hit::Item(id) = self.hit(x, y) else { return };
        let Some(index) = self.entries.iter().position(|e| e.id == id) else { return };
        let last = self.entries.len() - 1;
        let mut pt = POINT::default();
        // SAFETY: メニューは関数内で生成・破棄する
        let cmd = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 1, w!("編集 (次の段階で実装)"));
            let _ = AppendMenuW(menu, MF_STRING, 2, w!("削除"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING | if index == 0 { MF_GRAYED } else { MF_ENABLED }, 3, w!("上へ移動"));
            let _ = AppendMenuW(menu, MF_STRING | if index == last { MF_GRAYED } else { MF_ENABLED }, 4, w!("下へ移動"));
            let _ = GetCursorPos(&mut pt);
            self.modal += 1;
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, self.hwnd, None);
            self.modal -= 1;
            let _ = DestroyMenu(menu);
            cmd.0
        };
        match cmd {
            2 => self.delete_item(id),
            3 | 4 => {
                if items::move_by(&mut self.entries, index, if cmd == 3 { -1 } else { 1 }) {
                    self.save();
                    self.relayout(true);
                }
            }
            _ => {}
        }
    }

    fn delete_item(&mut self, id: u64) {
        let Some(e) = self.entry(id) else { return };
        let text = to_wide(&format!("「{}」を削除しますか？\n\n{}", e.item.name, e.item.path));
        self.modal += 1;
        // SAFETY: 文字列は呼び出し中有効
        let answer = unsafe {
            MessageBoxW(Some(self.hwnd), PCWSTR(text.as_ptr()), w!("アイテムの削除"), MB_OKCANCEL | MB_ICONQUESTION)
        };
        self.modal -= 1;
        if answer == IDOK {
            self.entries.retain(|e| e.id != id);
            self.save();
            self.relayout(true);
        }
    }

    fn on_drop_files(&mut self, hdrop: HDROP) {
        let mut paths = Vec::new();
        // SAFETY: WM_DROPFILES の HDROP を読んで解放する
        unsafe {
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                paths.push(String::from_utf16_lossy(&buf[..len]));
            }
            DragFinish(hdrop);
        }
        let mut list: Vec<Item> = self.entries.iter().map(|e| e.item.clone()).collect();
        let before = list.len();
        if items::add_paths(&mut list, paths) == 0 {
            self.show_footer("既に登録済みです");
            return;
        }
        for item in list.into_iter().skip(before) {
            self.push_entry(item);
        }
        self.save();
        self.relayout(false);
    }

    // ───────────── ドラッグ並び替え ─────────────

    fn update_drag(&mut self, x: f32, y: f32) {
        let area = self.item_area();
        let Some(drag) = &mut self.drag else { return };
        drag.pointer = (x, y);

        // アイテム領域の上下端に近ければ自動スクロール
        let max = (self.content_height - area.h).max(0.0);
        if y < area.y + AUTO_SCROLL_MARGIN {
            self.scroll_target = (self.scroll_target - 8.0).max(0.0);
        } else if y > area.y + area.h - AUTO_SCROLL_MARGIN {
            self.scroll_target = (self.scroll_target + 8.0).min(max);
        }

        // 対象ボタンの上半分なら前、下半分なら後へ挿入
        let (cx, cy) = (x, y - TOP_BAR + self.scroll);
        if let Some(&(target, slot)) = self.slots.iter().find(|(_, r)| r.contains(cx, cy)) {
            let drag = self.drag.as_mut().unwrap();
            if target != drag.id {
                let after = cy >= slot.y + slot.h / 2.0;
                let order = items::reorder(&drag.order, drag.id, target, after);
                if order != drag.order {
                    drag.order = order;
                    self.relayout(true);
                }
            }
        }
        self.invalidate();
    }

    /// 左ボタンを離した時点の順序を確定して保存する。掴んでいたボタンは
    /// 追従表示の位置から新しい場所へ滑らかに収まる。
    fn finish_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        self.press = None;
        let changed = drag.order != self.entries.iter().map(|e| e.id).collect::<Vec<_>>();
        let floating = (drag.pointer.0 - drag.grab.0, drag.pointer.1 - drag.grab.1 - TOP_BAR + self.scroll);
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == drag.id) {
            e.pos = floating;
        }
        if changed {
            self.entries.sort_by_key(|e| drag.order.iter().position(|&id| id == e.id).unwrap_or(usize::MAX));
            self.save();
        }
        self.relayout(true);
    }

    // ───────────── リサイズ ─────────────

    fn update_resize(&mut self) {
        let Some(op) = &self.resize else { return };
        let mut cursor = POINT::default();
        // SAFETY: カーソル位置の取得
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            return;
        }
        let (dx, dy) = (cursor.x - op.cursor.x, cursor.y - op.cursor.y);
        let min = (MIN_INNER_SIZE as f32 * self.scale()).ceil() as i32;
        let r = op.rect;
        let (mut x, mut y, mut w, mut h) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
        if op.dir & DIR_LEFT != 0 {
            w = (r.right - r.left - dx).max(min);
            x = r.right - w;
        } else if op.dir & DIR_RIGHT != 0 {
            w = (r.right - r.left + dx).max(min);
        }
        if op.dir & DIR_TOP != 0 {
            h = (r.bottom - r.top - dy).max(min);
            y = r.bottom - h;
        } else if op.dir & DIR_BOTTOM != 0 {
            h = (r.bottom - r.top + dy).max(min);
        }
        // SAFETY: 自分のウィンドウの移動・リサイズ (WM_SIZE で再レイアウトと描画が行われる)
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.paint();
    }

    /// 利用者のリサイズ完了時だけ、幅を列単位へスナップして内側サイズを保存する。
    fn finish_resize(&mut self, dir: u8) {
        let scale = self.scale() as f64;
        let mut rc = RECT::default();
        // SAFETY: 自分のウィンドウ矩形の取得
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rc);
        }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let column = self.settings.window.width;
        let columns = layout::column_count(w as f64 / scale, column, self.entries.len());
        let target_dip = layout::snapped_width(columns, column);
        let target_px = (target_dip * scale).round() as i32;
        if target_px != w {
            // 左辺で操作した場合は右端を固定してスナップする
            let x = if dir & DIR_LEFT != 0 { rc.right - target_px } else { rc.left };
            // SAFETY: 自分のウィンドウのリサイズ
            unsafe {
                let _ = SetWindowPos(self.hwnd, None, x, rc.top, target_px, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.settings.window.inner_width = Some(target_dip);
        self.settings.window.inner_height = Some(((h as f64 / scale) * 100.0).round() / 100.0).map(|v| v.max(MIN_INNER_SIZE));
        self.save();
    }

    // ───────────── 保存・通知 ─────────────

    fn save(&mut self) {
        let config = Config { settings: self.settings.clone(), items: self.entries.iter().map(|e| e.item.clone()).collect() };
        if !self.store.save(&config) {
            self.show_footer("設定を保存できませんでした (LaunchPanel.log を参照)");
        }
    }

    fn show_footer(&mut self, message: &str) {
        self.footer = Some(message.to_owned());
        // SAFETY: タイマー設定
        unsafe { SetTimer(Some(self.hwnd), TIMER_FOOTER, 4000, None) };
        self.invalidate();
    }

    fn on_icons(&mut self) {
        for (id, pixels) in self.icons.drain() {
            if id == FALLBACK_ICON_ID {
                self.fallback = pixels;
                self.fallback_bitmap = None;
            } else if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                e.icon_failed = pixels.is_none();
                e.icon = pixels;
                e.bitmap = None;
            }
        }
        self.invalidate();
    }

    // ───────────── 描画 ─────────────

    /// 位置とスクロールを目標へ近づける。まだ動いていれば true。
    fn step_animation(&mut self) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        let mut moving = false;
        if self.animate_moves {
            let k = 1.0 - (-dt / MOVE_TAU).exp();
            for e in &mut self.entries {
                let (dx, dy) = (e.target.0 - e.pos.0, e.target.1 - e.pos.1);
                if dx.abs() > 0.2 || dy.abs() > 0.2 {
                    e.pos.0 += dx * k;
                    e.pos.1 += dy * k;
                    moving = true;
                } else {
                    e.pos = e.target;
                }
            }
            if !moving && self.drag.is_none() {
                self.animate_moves = false;
            }
        }
        let ds = self.scroll_target - self.scroll;
        if ds.abs() > 0.2 {
            self.scroll += ds * (1.0 - (-dt / SCROLL_TAU).exp());
            moving = true;
        } else {
            self.scroll = self.scroll_target;
        }
        moving || self.drag.is_some()
    }

    pub fn paint(&mut self) {
        if self.gfx.ensure_target(self.hwnd).is_err() {
            return;
        }
        let animating = self.step_animation();
        self.refresh_bitmaps();

        let gfx = &self.gfx;
        gfx.begin(BG);
        let full = Rect::new(0.0, 0.0, self.width, self.height);
        if let Some((_, bg)) = &self.background {
            gfx.bitmap_cover(bg, full);
        }

        // 上部: サイズと列数、設定、ピン留め
        let status = layout::status_text(self.width as f64, self.height as f64, self.column_count());
        let sw = gfx.measure_small(&status) + 16.0;
        gfx.fill_round(Rect::new(8.0, 8.0, sw, 24.0), 6.0, PILL);
        gfx.text(&status, Rect::new(8.0, 8.0, sw, 24.0), OVERLAY_TEXT, TextStyle::Small);
        for (rect, hit, glyph) in [(self.gear_rect(), Hit::Gear, "\u{E713}"), (self.pin_rect(), Hit::Pin, "")] {
            gfx.fill_round(rect, 6.0, if self.hover == hit { PILL_HOVER } else { PILL });
            if hit == Hit::Pin {
                // 未固定は傾いたピン、固定は直立したピン (色だけに頼らず向きで状態を示す)
                let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                if !self.pinned {
                    gfx.rotate(45.0, cx, cy);
                }
                gfx.text(if self.pinned { "\u{E840}" } else { "\u{E718}" }, rect, OVERLAY_TEXT, TextStyle::Glyph);
                gfx.reset_transform();
            } else {
                gfx.text(glyph, rect, OVERLAY_TEXT, TextStyle::Glyph);
            }
        }

        // アイテム
        let area = self.item_area();
        gfx.push_clip(area);
        let dragging = self.drag.as_ref().map(|d| d.id);
        for e in &self.entries {
            let r = Rect::new(e.pos.0, area.y + e.pos.1 - self.scroll, e.width, BUTTON_H);
            if r.y + r.h < area.y || r.y > area.y + area.h {
                continue;
            }
            let faded = dragging == Some(e.id);
            let hovered = self.hover == Hit::Item(e.id) && !faded;
            self.draw_button(e, r, hovered, if faded { 0.35 } else { 1.0 });
        }
        if self.content_height > area.h {
            // スクロール位置の目安
            let ratio = area.h / self.content_height;
            let bar_h = (area.h * ratio).max(24.0);
            let bar_y = area.y + (area.h - bar_h) * (self.scroll / self.max_scroll().max(1.0));
            gfx.fill_round(Rect::new(self.width - 5.0, bar_y, 3.0, bar_h), 1.5, Color::rgba(255, 255, 255, 0x50));
        }
        gfx.pop_clip();

        // 下部の案内
        let footer = self.footer.as_deref().unwrap_or(FOOTER_HINT);
        let fw = (gfx.measure_small(footer) + 16.0).min(self.width - 16.0);
        let fr = Rect::new((self.width - fw) / 2.0, self.height - 8.0 - 24.0, fw, 24.0);
        gfx.fill_round(fr, 6.0, PILL);
        gfx.text(footer, fr.inset(6.0, 0.0), OVERLAY_TEXT, TextStyle::Small);

        // ドラッグ中のボタン (掴んだ位置関係を保ってカーソルへ追従)
        if let Some(d) = &self.drag {
            if let Some(e) = self.entry(d.id) {
                let r = Rect::new(d.pointer.0 - d.grab.0, d.pointer.1 - d.grab.1, e.width, BUTTON_H);
                self.draw_button(e, r, true, 0.92);
            }
        }
        self.gfx.end();

        if animating {
            self.invalidate();
        }
    }

    fn draw_button(&self, e: &Entry, r: Rect, hovered: bool, opacity: f32) {
        let s = &self.settings.item_button;
        let gfx = &self.gfx;
        let bg = s.background_color;
        gfx.fill_round(r, 5.0, Color::rgba(bg.0, bg.1, bg.2, 255).with_alpha(s.background_alpha() * opacity));
        let bitmap = e.bitmap.as_ref().map(|(_, b)| b).or(if e.icon_failed || e.icon.is_none() {
            self.fallback_bitmap.as_ref().map(|(_, b)| b)
        } else {
            None
        });
        if let Some(b) = bitmap {
            gfx.bitmap(b, Rect::new(r.x + 8.0, r.y + (BUTTON_H - ICON) / 2.0, ICON, ICON), opacity);
        }
        let tc = s.text_color;
        let style = if s.bold_text { TextStyle::ItemBold } else { TextStyle::Item };
        gfx.text(
            &e.item.name,
            Rect::new(r.x + 36.0, r.y, (r.w - 44.0).max(0.0), BUTTON_H),
            Color::rgba(tc.0, tc.1, tc.2, 255).with_alpha(opacity),
            style,
        );
        if hovered {
            // 明るい輪郭と薄い白の重ねで、背景の明暗によらず識別できるようにする
            gfx.fill_round(r, 5.0, HOVER_FILL.with_alpha(HOVER_FILL.3 * opacity));
            gfx.stroke_round(r, 5.0, HOVER_OUTLINE.with_alpha(opacity), 2.0);
        }
    }

    /// レンダーターゲットの世代が変わったビットマップを作り直す。
    fn refresh_bitmaps(&mut self) {
        let generation = self.gfx.generation;
        for e in &mut self.entries {
            if e.bitmap.as_ref().is_none_or(|(g, _)| *g != generation) {
                e.bitmap = e.icon.as_ref().and_then(|p| self.gfx.create_bitmap(p)).map(|b| (generation, b));
            }
        }
        if self.fallback_bitmap.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.fallback_bitmap = self.fallback.as_ref().and_then(|p| self.gfx.create_bitmap(p)).map(|b| (generation, b));
        }
        if self.background_tried != generation {
            self.background_tried = generation;
            let path = self.settings.background_image.trim();
            self.background = if path.is_empty() {
                None
            } else {
                let loaded = self.gfx.load_image(path);
                if loaded.is_none() {
                    crate::log::write(&format!("背景画像を読み込めません: {path}"));
                }
                loaded.map(|b| (generation, b))
            };
        }
    }

    // ───────────── メッセージ ─────────────

    pub fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let point = || {
            let s = self.scale();
            ((lparam.0 & 0xFFFF) as i16 as f32 / s, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / s)
        };
        match msg {
            WM_PAINT => {
                self.paint();
                // SAFETY: 描画済みとして領域を確定する
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
            }
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SIZE => {
                let (w, h) = ((lparam.0 & 0xFFFF) as u32, ((lparam.0 >> 16) & 0xFFFF) as u32);
                self.gfx.resize(w, h);
                let s = self.scale();
                self.width = w as f32 / s;
                self.height = h as f32 / s;
                self.relayout(self.drag.is_some());
            }
            WM_DPICHANGED => {
                // SAFETY: lParam は推奨矩形
                let r = unsafe { &*(lparam.0 as *const RECT) };
                self.gfx.set_dpi((wparam.0 & 0xFFFF) as u32);
                // SAFETY: 自分のウィンドウの移動
                unsafe {
                    let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let mut pt = POINT::default();
                // SAFETY: カーソル位置の変換
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
                }
                let s = self.scale();
                let cursor = if self.drag.is_some() {
                    IDC_SIZEALL
                } else {
                    match self.resize.as_ref().map(|r| r.dir).map(Hit::Resize).unwrap_or(self.hit(pt.x as f32 / s, pt.y as f32 / s)) {
                        Hit::Resize(d) if d == DIR_LEFT || d == DIR_RIGHT => IDC_SIZEWE,
                        Hit::Resize(d) if d == DIR_TOP || d == DIR_BOTTOM => IDC_SIZENS,
                        Hit::Resize(d) if d == DIR_LEFT | DIR_TOP || d == DIR_RIGHT | DIR_BOTTOM => IDC_SIZENWSE,
                        Hit::Resize(_) => IDC_SIZENESW,
                        _ => IDC_ARROW,
                    }
                };
                // SAFETY: システムカーソルの設定
                unsafe {
                    let _ = SetCursor(LoadCursorW(None, cursor).ok());
                }
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let (x, y) = point();
                self.on_mouse_move(x, y);
            }
            WM_MOUSELEAVE => {
                self.tracking_leave = false;
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.invalidate();
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point();
                self.on_left_down(x, y);
            }
            WM_LBUTTONUP => {
                let (x, y) = point();
                self.on_left_up(x, y);
            }
            WM_CAPTURECHANGED => self.on_capture_lost(),
            WM_RBUTTONUP => {
                let (x, y) = point();
                self.on_right_up(x, y);
            }
            WM_MOUSEWHEEL => self.on_wheel(((wparam.0 >> 16) & 0xFFFF) as i16),
            WM_DROPFILES => self.on_drop_files(HDROP(wparam.0 as *mut _)),
            WM_ACTIVATE => {
                crate::log::debug(&format!(
                    "WM_ACTIVATE {} (相手={}, 復帰から {} ms)",
                    wparam.0 & 0xFFFF,
                    window_label(HWND(lparam.0 as *mut _)),
                    self.shown_at.elapsed().as_millis()
                ));
                if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                    self.auto_hide();
                }
            }
            WM_TIMER => {
                // SAFETY: 自分のタイマーの停止
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), wparam.0);
                }
                match wparam.0 {
                    TIMER_GRACE => self.auto_hide(),
                    TIMER_FOOTER => {
                        self.footer = None;
                        self.invalidate();
                    }
                    _ => {}
                }
            }
            WM_CLOSE => {
                if !self.exiting {
                    // 閉じる操作は終了ではなく非表示。ピン留めも解除する
                    self.pinned = false;
                    self.hide();
                    return Some(LRESULT(0));
                }
            }
            WM_DESTROY => {
                // SAFETY: メッセージループの終了
                unsafe { PostQuitMessage(0) };
            }
            WM_APP_SHOW => self.show(),
            WM_APP_ICON => self.on_icons(),
            WM_APP_DESKTOP => self.show_at(wparam.0 as i32, lparam.0 as i32),
            WM_APP_SHELL => match wparam.0 as u32 {
                1 | 4 => self.show(),
                2 => {
                    self.show();
                    self.show_footer("設定画面は次の段階で実装予定です");
                }
                3 => self.exit(),
                _ => {}
            },
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// 診断ログ用のウィンドウ表記 (クラス名)。
fn window_label(hwnd: HWND) -> String {
    if hwnd.is_invalid() {
        return "なし".into();
    }
    let mut buf = [0u16; 64];
    // SAFETY: バッファ長はスライスで渡す
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// モニターの作業領域 (物理座標)。
fn work_area(pt: POINT, flags: windows::Win32::Graphics::Gdi::MONITOR_FROM_FLAGS) -> RECT {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: モニター情報の取得
    unsafe {
        let monitor = MonitorFromPoint(pt, flags);
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    info.rcWork
}

// ───────────── ネイティブ側からの通知 ─────────────

static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn set_main_hwnd(hwnd: HWND) {
    MAIN_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
}

fn post(msg: u32, wparam: usize, lparam: isize) {
    let hwnd = MAIN_HWND.load(std::sync::atomic::Ordering::SeqCst);
    if hwnd != 0 {
        // SAFETY: メッセージの送信のみ (UI スレッドで処理される)
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(wparam), LPARAM(lparam));
        }
    }
}

extern "system" fn on_shell_event(event: u32) {
    post(WM_APP_SHELL, event as usize, 0);
}

extern "system" fn on_desktop_double_click(x: i32, y: i32) {
    post(WM_APP_DESKTOP, x as usize, y as isize);
}
