//! メインウィンドウ。状態・レイアウト・描画・入力処理・アニメーション。
//!
//! 座標は特記しない限りクライアント領域の DIP。アイテムの位置は「コンテンツ座標」
//! (アイテム領域の左上原点、スクロール前) で持ち、描画時にスクロール量を引く。
//! アニメーションは描画のたびに目標値へ指数的に近づけ、動いている間だけ次のフレームを要求する。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
    HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MonitorFromRect,
    ValidateRect,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::HDROP;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::{BUTTON_COLORS, Config, Item, MIN_INNER_SIZE, Rgb, Settings, Store};
use crate::platform::icon::Pixels;
use crate::platform::shell::{self, ShellEvent};
use crate::platform::wide::to_wide;
use crate::platform::{desktop, foreground, shadow, window};
use crate::render::{CachedBitmap, Color, Gfx, Rect, TextStyle};
use crate::services::IconLoader;
use crate::{items, layout};

// ───────────── 定数 ─────────────

pub const WM_APP_SHOW: u32 = WM_APP + 10;
pub const WM_APP_SHELL: u32 = WM_APP + 11;
pub const WM_APP_ICON: u32 = WM_APP + 12;
pub const WM_APP_DESKTOP: u32 = WM_APP + 13;

const TIMER_GRACE: usize = 1;
const TIMER_FOOTER: usize = 2;
/// 表示中だけ動かす見張り (フォーカス喪失の通知を取りこぼしても隠せるように)
const TIMER_WATCH: usize = 3;
/// 前面化に失敗した時のやり直し
const TIMER_RETRY: usize = 4;
const WATCH_INTERVAL_MS: u32 = 500;
const RETRY_INTERVAL_MS: u32 = 120;
const MAX_RETRIES: u32 = 3;

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

const PILL: Color = Color::rgba(0x14, 0x15, 0x18, 0xB0);
const PILL_HOVER: Color = Color::rgba(0x3A, 0x3C, 0x44, 0xE0);
const OVERLAY_TEXT: Color = Color::rgba(0xE8, 0xE8, 0xEC, 0xFF);

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
    bitmap: CachedBitmap,
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
    fallback_bitmap: CachedBitmap,
    backdrop: crate::appearance::BackdropView,
    /// 設定画面で試し表示中の背景 (無ければ保存済みの設定)
    preview_look: Option<crate::appearance::Look>,

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
    settings_open: bool,
    shown_at: Instant,
    /// 復帰直後の猶予中に前面を奪われた。猶予の終わりに取り戻す
    reclaim_focus: bool,
    /// 表示してから一度でもアクティブになったか
    activated: bool,
    /// 前面化に失敗した時の前面ウィンドウ。アクティブになるまでは、前面がこれのままなら隠さない
    /// (管理者権限のアプリが前面の時など。出た瞬間に消えないように)
    fg_baseline: isize,
    retries: u32,
    /// 前面化に失敗したので最前面に置いている
    topmost: bool,
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
            backdrop: Default::default(),
            preview_look: None,
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
            settings_open: false,
            shown_at: Instant::now(),
            reclaim_focus: false,
            activated: false,
            fg_baseline: 0,
            retries: 0,
            topmost: false,
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
        window::scale(self.hwnd)
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
        self.apply_look();

        let h = &self.settings.hotkey;
        let modifiers =
            (h.ctrl as u32 * 2) | (h.alt as u32) | (h.shift as u32 * 4) | (h.win as u32 * 8);
        if !shell::start(on_shell_event, modifiers, h.key) {
            crate::log::write(&format!(
                "ホットキーの登録に失敗しました: {modifiers:#x}+{} (他のアプリが使用中の可能性があります)",
                h.key
            ));
        }
        if self.settings.desktop_double_click && !desktop::start(on_desktop_double_click) {
            crate::log::write("デスクトップのダブルクリック監視を開始できませんでした。");
        }
        self.show("起動");
    }

    /// 保存済みの内側サイズで、プライマリモニターの作業領域中央に置く。
    fn place_initially(&mut self) {
        let scale = self.scale() as f64;
        let w = (self.settings.window.effective_inner_width() * scale).round() as i32;
        let h = (self.settings.window.effective_inner_height() * scale).round() as i32;
        let area = window::work_area_at(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let x = area.left + ((area.right - area.left - w) / 2).max(0);
        let y = area.top + ((area.bottom - area.top - h) / 2).max(0);
        // SAFETY: 自分のウィンドウの移動
        let _ = unsafe { SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    fn apply_shadow(&self) {
        let w = &self.settings.window;
        shadow::set_window_shadow(self.hwnd, w.shadow, w.shadow_opacity);
    }

    /// 従来の位置で復帰し、前面化して入力フォーカスを得る。`reason` は診断ログ用の呼び出し元。
    pub fn show(&mut self, reason: &str) {
        self.ensure_on_screen();
        self.shown_at = Instant::now();
        self.reclaim_focus = false;
        self.activated = false;
        // SAFETY: 前面ウィンドウの参照と自分のウィンドウの表示
        let (before, ok) = unsafe {
            let before = GetForegroundWindow();
            let cmd = if foreground::simulate_failure() {
                SW_SHOWNOACTIVATE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(self.hwnd, cmd);
            (before, foreground::force_foreground(self.hwnd))
        };
        crate::log::write(&format!(
            "表示 ({reason}): 前面化{} (直前の前面={})",
            if ok { "成功" } else { "失敗" },
            window_label(before)
        ));
        if ok {
            self.set_topmost(false);
        } else {
            self.foreground_failed(before);
        }
        self.set_timer(TIMER_WATCH, WATCH_INTERVAL_MS);
        self.invalidate();
    }

    /// 前面化できなかった。最前面に置いて見えるようにし、何回かやり直す。
    /// `baseline` が前面のままの間は見張りでも隠さない (クリックすれば普通に使える)。
    fn foreground_failed(&mut self, baseline: HWND) {
        self.activated = false;
        self.fg_baseline = baseline.0 as isize;
        self.retries = 0;
        self.set_topmost(true);
        self.set_timer(TIMER_RETRY, RETRY_INTERVAL_MS);
    }

    fn retry_foreground(&mut self) {
        if self.activated || !self.is_visible() {
            return;
        }
        self.retries += 1;
        if foreground::force_foreground(self.hwnd) {
            crate::log::write(&format!("前面化をやり直して成功 ({} 回目)", self.retries));
            self.set_topmost(false);
        } else if self.retries < MAX_RETRIES {
            self.set_timer(TIMER_RETRY, RETRY_INTERVAL_MS);
        } else {
            crate::log::write(&format!(
                "前面化できないため、最前面に表示してクリックを待つ (前面={})",
                // SAFETY: 前面ウィンドウの参照
                window_label(unsafe { GetForegroundWindow() })
            ));
        }
    }

    /// 前面化に失敗した間だけ最前面に置く (自分のウィンドウの Z 順は前面化の制限を受けない)。
    fn set_topmost(&mut self, on: bool) {
        if self.topmost == on {
            return;
        }
        self.topmost = on;
        // SAFETY: 自分のウィンドウの Z 順の変更
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// ウィンドウがどのモニターの作業領域にもほとんど入っていなければ (モニターを外した、
    /// 画面構成が変わった等)、最も近いモニターの作業領域へ収める。
    fn ensure_on_screen(&mut self) {
        let rc = self.window_rect();
        // SAFETY: モニターの検索のみ
        let area = window::work_area(unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST) });
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let visible_w = rc.right.min(area.right) - rc.left.max(area.left);
        let visible_h = rc.bottom.min(area.bottom) - rc.top.max(area.top);
        if visible_w >= w.min(120) && visible_h >= h.min(60) {
            return;
        }
        let (x, y) = window::clamp_into(rc.left, rc.top, w, h, area);
        crate::log::write(&format!(
            "画面外にあったため移動: ({}, {}) → ({x}, {y})",
            rc.left, rc.top
        ));
        window::move_to(self.hwnd, x, y);
    }

    /// ウィンドウの矩形 (物理スクリーン座標)。
    fn window_rect(&self) -> RECT {
        let mut rc = RECT::default();
        // SAFETY: 自分のウィンドウの矩形取得
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rc);
        }
        rc
    }

    /// 表示中の見張り。前面が他のアプリになっていれば隠す。
    /// フォーカス喪失の通知 (WM_ACTIVATE) を、メニューやダイアログの表示中・リサイズ中・
    /// 前面化の失敗などで取りこぼしても、ここで回収する。
    fn watch(&mut self) {
        if self.exiting || !self.is_visible() {
            self.kill_timer(TIMER_WATCH);
            return;
        }
        if self.keeps_shown()
            || self.press.is_some()
            || self.drag.is_some()
            || self.reclaim_focus
            || self.shown_at.elapsed() < RESTORE_GRACE * 2
        {
            return;
        }
        // SAFETY: 前面ウィンドウの参照
        let fg = unsafe { GetForegroundWindow() };
        // 切り替えの途中 (前面なし) と、自分のプロセスの窓 (ダイアログ・メニュー) は除く
        if fg.is_invalid() || is_own_window(fg) {
            return;
        }
        if !self.activated && fg.0 as isize == self.fg_baseline {
            return;
        }
        self.hide(&format!("見張り: 前面が他のアプリ ({})", window_label(fg)));
    }

    /// デスクトップのダブルクリック位置 (物理座標) へ左上を合わせて復帰する。
    /// 非表示のまま移動してから表示するので、以前の位置は見えない。
    fn show_at(&mut self, x: i32, y: i32) {
        crate::log::debug(&format!("デスクトップのダブルクリックで復帰: ({x}, {y})"));
        let rc = self.window_rect();
        let area = window::work_area_at(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (x, y) = window::clamp_into(x, y, rc.right - rc.left, rc.bottom - rc.top, area);
        window::move_to(self.hwnd, x, y);
        self.show("デスクトップのダブルクリック");
    }

    fn hide(&mut self, reason: &str) {
        crate::log::write(&format!("非表示 ({reason})"));
        self.set_topmost(false);
        self.kill_timer(TIMER_WATCH);
        self.kill_timer(TIMER_RETRY);
        // SAFETY: 自分のウィンドウの非表示
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn is_visible(&self) -> bool {
        // SAFETY: ウィンドウ状態の参照
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// 自動非表示しない状態 (ピン留め中・ダイアログやメニューの表示中・リサイズ中・終了処理中)。
    fn keeps_shown(&self) -> bool {
        self.pinned || self.exiting || self.modal > 0 || self.resize.is_some()
    }

    fn set_timer(&self, id: usize, ms: u32) {
        // SAFETY: 自分のウィンドウのタイマー設定
        unsafe { SetTimer(Some(self.hwnd), id, ms, None) };
    }

    fn kill_timer(&self, id: usize) {
        // SAFETY: 自分のウィンドウのタイマー停止
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    /// ピン留め中・ダイアログ表示中でなく、前面でもなければ非表示にする。
    fn auto_hide(&mut self) {
        if self.keeps_shown() || !self.is_visible() {
            return;
        }
        let elapsed = self.shown_at.elapsed();
        if elapsed < RESTORE_GRACE {
            // 復帰直後のフォーカス遷移 (デスクトップのダブルクリックを Explorer が後から処理して
            // 前面を取り返す等) はフォーカス喪失と見なさない。猶予の終わりに前面を取り戻す
            crate::log::debug(&format!(
                "復帰直後に非アクティブ化 ({} ms)。猶予後に前面を取り戻す",
                elapsed.as_millis()
            ));
            self.reclaim_focus = true;
            self.set_timer(
                TIMER_GRACE,
                (RESTORE_GRACE - elapsed).as_millis() as u32 + 1,
            );
            return;
        }
        if std::mem::take(&mut self.reclaim_focus) {
            // SAFETY: 前面ウィンドウの参照と前面化
            unsafe {
                let fg = GetForegroundWindow();
                if fg != self.hwnd {
                    let ok = foreground::force_foreground(self.hwnd);
                    crate::log::write(&format!(
                        "復帰直後に前面を奪われたので取り戻す: {} (前面={})",
                        if ok { "成功" } else { "失敗" },
                        window_label(fg)
                    ));
                    if !ok {
                        self.foreground_failed(fg);
                    }
                }
            }
            return;
        }
        // SAFETY: 前面ウィンドウの参照
        let fg = unsafe { GetForegroundWindow() };
        if fg == self.hwnd {
            return;
        }
        self.hide(&format!("フォーカス喪失 (前面={})", window_label(fg)));
    }

    fn exit(&mut self) {
        self.exiting = true;
        desktop::stop();
        shell::stop();
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
        Rect::new(
            0.0,
            TOP_BAR,
            self.width,
            (self.height - TOP_BAR - FOOTER).max(0.0),
        )
    }

    fn column_count(&self) -> usize {
        layout::column_count(
            self.width as f64,
            self.settings.window.width,
            self.entries.len(),
        )
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
            self.slots.push((
                id,
                Rect::new(c as f32 * column_w, r as f32 * ROW_H, column_w, ROW_H),
            ));
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
        let (on_left, on_right, on_top, on_bottom) =
            (x < EDGE, x >= w - EDGE, y < EDGE, y >= h - EDGE);
        if on_left || on_right || on_top || on_bottom {
            // 端の帯の上では、角から CORNER 以内を斜め方向として扱う (角は辺より広く掴める)
            let (near_left, near_right, near_top, near_bottom) =
                (x < CORNER, x >= w - CORNER, y < CORNER, y >= h - CORNER);
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
        if let Some(p) = &self.press
            && self.drag.is_none()
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
        if self.drag.is_some() {
            self.update_drag(x, y);
            return;
        }

        if !self.tracking_leave {
            self.tracking_leave = window::track_leave(self.hwnd);
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
                    SendMessageW(
                        self.hwnd,
                        WM_NCLBUTTONDOWN,
                        Some(WPARAM(HTCAPTION as usize)),
                        Some(LPARAM(0)),
                    );
                }
            }
            Hit::Item(id) => {
                let Some(e) = self.entry(id) else { return };
                let grab = (x - e.pos.0, y - (TOP_BAR + e.pos.1 - self.scroll));
                self.press = Some(Press {
                    id,
                    start: (x, y),
                    grab,
                });
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
            if self.hit(x, y) == Hit::Item(p.id)
                && let Some(e) = self.entry(p.id)
            {
                let path = e.item.path.clone();
                let name = e.item.name.clone();
                if !crate::services::launch(self.hwnd, &path) {
                    self.show_footer(&format!("起動できません: {name}"));
                }
            }
            return;
        }
        match self.hit(x, y) {
            Hit::Pin => {
                self.pinned = !self.pinned;
                self.invalidate();
            }
            Hit::Gear => self.open_settings(),
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
        self.scroll_target =
            (self.scroll_target - delta as f32 / 120.0 * ROW_H * 1.5).clamp(0.0, self.max_scroll());
        self.invalidate();
    }

    fn on_right_up(&mut self, x: f32, y: f32) {
        let Hit::Item(id) = self.hit(x, y) else {
            return;
        };
        let Some(index) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        let last = self.entries.len() - 1;
        let current_color = self.entries[index].item.color;
        let swatch_px = (16.0 * self.scale()).round() as i32;
        let mut pt = POINT::default();
        let mut swatches = Vec::new();
        // SAFETY: メニューと色見本のビットマップは関数内で生成・破棄する
        let cmd = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(menu, MF_STRING, 1, w!("編集..."));
            let _ = AppendMenuW(menu, MF_STRING, 2, w!("削除"));
            // ボタンの色 (標準 + グループの色)。色見本付きで、今の色に印を付ける
            if let Ok(colors) = CreatePopupMenu() {
                for i in 0..BUTTON_COLORS {
                    // 色見本が印の欄に描かれてチェックが見えないので、今の色は名前に印を付ける
                    let label = to_wide(&if i == current_color {
                        format!("{}  ✓", color_label(i))
                    } else {
                        color_label(i)
                    });
                    let bitmap = menu_swatch(self.settings.item_button.color(i), swatch_px);
                    swatches.extend(bitmap);
                    let info = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_STATE | MIIM_BITMAP,
                        fType: MFT_RADIOCHECK,
                        fState: if i == current_color {
                            MFS_CHECKED
                        } else {
                            MFS_UNCHECKED
                        },
                        wID: COLOR_COMMAND + i as u32,
                        dwTypeData: windows::core::PWSTR(label.as_ptr() as *mut _),
                        hbmpItem: bitmap.unwrap_or_default(),
                        ..Default::default()
                    };
                    let _ = InsertMenuItemW(colors, i as u32, true, &info);
                }
                let _ = AppendMenuW(menu, MF_POPUP, colors.0 as usize, w!("ボタンの色"));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING | if index == 0 { MF_GRAYED } else { MF_ENABLED },
                3,
                w!("上へ移動"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING | if index == last { MF_GRAYED } else { MF_ENABLED },
                4,
                w!("下へ移動"),
            );
            let _ = GetCursorPos(&mut pt);
            self.modal += 1;
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            self.modal -= 1;
            let _ = DestroyMenu(menu);
            for b in swatches {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
            cmd.0
        };
        match cmd {
            1 => self.open_edit(id),
            2 => self.delete_item(id),
            c if (COLOR_COMMAND as i32..COLOR_COMMAND as i32 + BUTTON_COLORS as i32)
                .contains(&c) =>
            {
                self.entries[index].item.color = (c - COLOR_COMMAND as i32) as usize;
                self.save();
                self.invalidate();
            }
            3 | 4 if items::move_by(&mut self.entries, index, if cmd == 3 { -1 } else { 1 }) => {
                self.save();
                self.relayout(true);
            }
            _ => {}
        }
    }

    fn delete_item(&mut self, id: u64) {
        let Some(e) = self.entry(id) else { return };
        let text = to_wide(&format!(
            "「{}」を削除しますか？\n\n{}",
            e.item.name, e.item.path
        ));
        self.modal += 1;
        // SAFETY: 文字列は呼び出し中有効
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(text.as_ptr()),
                w!("アイテムの削除"),
                MB_OKCANCEL | MB_ICONQUESTION,
            )
        };
        self.modal -= 1;
        if answer == IDOK {
            self.entries.retain(|e| e.id != id);
            self.save();
            self.relayout(true);
        }
    }

    fn on_drop_files(&mut self, hdrop: HDROP) {
        let paths = window::dropped_files(hdrop);
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
        let max = self.max_scroll();
        let Some(drag) = &mut self.drag else { return };
        drag.pointer = (x, y);

        // アイテム領域の上下端に近ければ自動スクロール
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
        let floating = (
            drag.pointer.0 - drag.grab.0,
            drag.pointer.1 - drag.grab.1 - TOP_BAR + self.scroll,
        );
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == drag.id) {
            e.pos = floating;
        }
        if changed {
            self.entries.sort_by_key(|e| {
                drag.order
                    .iter()
                    .position(|&id| id == e.id)
                    .unwrap_or(usize::MAX)
            });
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
            let x = if dir & DIR_LEFT != 0 {
                rc.right - target_px
            } else {
                rc.left
            };
            // SAFETY: 自分のウィンドウのリサイズ
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    x,
                    rc.top,
                    target_px,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        self.settings.window.inner_width = Some(target_dip);
        self.settings.window.inner_height =
            Some(((h as f64 / scale) * 100.0).round() / 100.0).map(|v| v.max(MIN_INNER_SIZE));
        self.save();
    }

    // ───────────── 編集ダイアログ ─────────────

    fn open_edit(&mut self, id: u64) {
        let Some(e) = self.entry(id) else { return };
        if crate::edit::open(self.hwnd, id, &e.item, &self.settings.item_button) {
            // 編集中は本体を自動非表示にしない
            self.modal += 1;
        }
    }

    fn on_edit_closed(&mut self, ok: bool) {
        self.modal = self.modal.saturating_sub(1);
        let Some(r) = crate::edit::take_result().filter(|_| ok) else {
            return;
        };
        let Some(e) = self.entries.iter_mut().find(|e| e.id == r.id) else {
            return;
        };
        let path_changed = e.item.path != r.path;
        e.item.name = r.name;
        e.item.path = r.path;
        e.item.color = r.color;
        if path_changed {
            // パスが変わったらアイコンを取り直す
            e.icon = None;
            e.icon_failed = false;
            e.bitmap = None;
            self.icons.request(e.id, &e.item.path);
        }
        self.save();
        self.invalidate();
    }

    // ───────────── 設定画面 ─────────────

    /// 設定画面を開く。既に開いていれば何もしない。
    fn open_settings(&mut self) {
        if self.settings_open {
            return;
        }
        self.settings_open =
            crate::settings::open(self.hwnd, &self.settings, self.fallback.as_ref());
        if self.settings_open {
            // 設定画面を操作している間は本体を自動非表示にしない
            self.modal += 1;
        }
    }

    fn on_settings_closed(&mut self, ok: bool) {
        self.settings_open = false;
        self.modal = self.modal.saturating_sub(1);
        match crate::settings::take_result().filter(|_| ok) {
            Some(new) => self.apply_settings(new),
            // キャンセル: 試し表示した影と背景を保存済みの値へ戻す
            None => {
                self.preview_look = None;
                self.apply_look();
                self.apply_shadow();
            }
        }
    }

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let scale = self.scale() as f64;
        if old.window.width != self.settings.window.width {
            // 現在の列数を維持したまま、新しい列幅に合わせてウィンドウ幅を変える
            let width = layout::width_for_column_width_change(
                self.width as f64,
                old.window.width,
                self.settings.window.width,
                self.entries.len(),
            );
            self.settings.window.inner_width = Some(width);
            self.settings
                .window
                .inner_height
                .get_or_insert((self.height as f64 * 100.0).round() / 100.0);
            let mut rc = RECT::default();
            // SAFETY: 自分のウィンドウのリサイズ
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut rc);
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    (width * scale).round() as i32,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOMOVE | SWP_NOACTIVATE,
                );
            }
        }
        self.preview_look = None;
        self.apply_look();
        self.apply_shadow();
        if old.desktop_double_click != self.settings.desktop_double_click {
            if self.settings.desktop_double_click {
                if !desktop::start(on_desktop_double_click) {
                    crate::log::write("デスクトップのダブルクリック監視を開始できませんでした。");
                }
            } else {
                desktop::stop();
            }
        }
        self.save();
        self.relayout(false);
        if old.hotkey != self.settings.hotkey {
            self.show_footer("ホットキーの変更は次回起動時に有効になります");
        }
    }

    // ───────────── 保存・通知 ─────────────

    fn save(&mut self) {
        let config = Config {
            settings: self.settings.clone(),
            items: self.entries.iter().map(|e| e.item.clone()).collect(),
        };
        if !self.store.save(&config) {
            self.show_footer("設定を保存できませんでした (LaunchPanel.log を参照)");
        }
    }

    fn show_footer(&mut self, message: &str) {
        self.footer = Some(message.to_owned());
        self.set_timer(TIMER_FOOTER, 4000);
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

        let look = self.look();
        let full = Rect::new(0.0, 0.0, self.width, self.height);
        self.gfx.begin(self.backdrop.clear_color());
        self.backdrop.draw(&self.gfx, self.hwnd, &look, full);
        let gfx = &self.gfx;

        // 上部: サイズと列数、設定、ピン留め
        let status =
            layout::status_text(self.width as f64, self.height as f64, self.column_count());
        let sw = gfx.measure_small(&status) + 16.0;
        gfx.fill_round(Rect::new(8.0, 8.0, sw, 24.0), 6.0, PILL);
        gfx.text(
            &status,
            Rect::new(8.0, 8.0, sw, 24.0),
            OVERLAY_TEXT,
            TextStyle::Small,
        );
        for (rect, hit, glyph) in [
            (self.gear_rect(), Hit::Gear, "\u{E713}"),
            (self.pin_rect(), Hit::Pin, ""),
        ] {
            gfx.fill_round(rect, 6.0, if self.hover == hit { PILL_HOVER } else { PILL });
            if hit == Hit::Pin {
                // 未固定は傾いたピン、固定は直立したピン (色だけに頼らず向きで状態を示す)
                let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                if !self.pinned {
                    gfx.rotate(45.0, cx, cy);
                }
                gfx.text(
                    if self.pinned { "\u{E840}" } else { "\u{E718}" },
                    rect,
                    OVERLAY_TEXT,
                    TextStyle::Glyph,
                );
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
            gfx.fill_round(
                Rect::new(self.width - 5.0, bar_y, 3.0, bar_h),
                1.5,
                Color::rgba(255, 255, 255, 0x50),
            );
        }
        gfx.pop_clip();

        // 下部の案内
        let footer = self.footer.as_deref().unwrap_or(FOOTER_HINT);
        let fw = (gfx.measure_small(footer) + 16.0).min(self.width - 16.0);
        let fr = Rect::new((self.width - fw) / 2.0, self.height - 8.0 - 24.0, fw, 24.0);
        gfx.fill_round(fr, 6.0, PILL);
        gfx.text(footer, fr.inset(6.0, 0.0), OVERLAY_TEXT, TextStyle::Small);

        // ドラッグ中のボタン (掴んだ位置関係を保ってカーソルへ追従)
        if let Some(d) = &self.drag
            && let Some(e) = self.entry(d.id)
        {
            let r = Rect::new(
                d.pointer.0 - d.grab.0,
                d.pointer.1 - d.grab.1,
                e.width,
                BUTTON_H,
            );
            self.draw_button(e, r, true, 0.92);
        }
        self.gfx.end();

        if animating {
            self.invalidate();
        }
    }

    fn draw_button(&self, e: &Entry, r: Rect, hovered: bool, opacity: f32) {
        // アイコンが取れなかったアイテムは LaunchPanel のアイコンで代用する
        let icon = e
            .bitmap
            .as_ref()
            .map(|(_, b)| b)
            .or(if e.icon_failed || e.icon.is_none() {
                self.fallback_bitmap.as_ref().map(|(_, b)| b)
            } else {
                None
            });
        crate::ui::item_button(
            &self.gfx,
            r,
            &e.item.name,
            icon,
            &self.settings.item_button,
            e.item.color,
            hovered,
            opacity,
        );
    }

    /// レンダーターゲットの世代が変わったビットマップを作り直す。
    fn refresh_bitmaps(&mut self) {
        for e in &mut self.entries {
            self.gfx.refresh_bitmap(&mut e.bitmap, e.icon.as_ref());
        }
        self.gfx
            .refresh_bitmap(&mut self.fallback_bitmap, self.fallback.as_ref());
    }

    /// 現在の背景の描き方 (設定画面の試し表示中は編集中の値)。
    fn look(&self) -> crate::appearance::Look {
        self.preview_look
            .clone()
            .unwrap_or_else(|| crate::appearance::Look::of(&self.settings))
    }

    /// アクリルの有無を背景の設定に合わせ、描き直す。
    fn apply_look(&mut self) {
        let look = self.look();
        self.backdrop.update_mode(self.hwnd, &look);
        self.invalidate();
    }

    // ───────────── メッセージ ─────────────

    pub fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let point = || window::point(self.hwnd, lparam);
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
                let (w, h) = window::client_size(lparam);
                self.gfx.resize(w, h);
                let s = self.scale();
                self.width = w as f32 / s;
                self.height = h as f32 / s;
                self.relayout(self.drag.is_some());
            }
            WM_DPICHANGED => {
                self.gfx.set_dpi((wparam.0 & 0xFFFF) as u32);
                // SAFETY: WM_DPICHANGED の lParam
                unsafe { window::apply_suggested_rect(self.hwnd, lparam) };
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let (x, y) = window::cursor(self.hwnd);
                let cursor = if self.drag.is_some() {
                    IDC_SIZEALL
                } else {
                    match self
                        .resize
                        .as_ref()
                        .map(|r| r.dir)
                        .map(Hit::Resize)
                        .unwrap_or(self.hit(x, y))
                    {
                        Hit::Resize(d) if d == DIR_LEFT || d == DIR_RIGHT => IDC_SIZEWE,
                        Hit::Resize(d) if d == DIR_TOP || d == DIR_BOTTOM => IDC_SIZENS,
                        Hit::Resize(d)
                            if d == DIR_LEFT | DIR_TOP || d == DIR_RIGHT | DIR_BOTTOM =>
                        {
                            IDC_SIZENWSE
                        }
                        Hit::Resize(_) => IDC_SIZENESW,
                        _ => IDC_ARROW,
                    }
                };
                window::set_cursor(cursor);
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
                } else {
                    self.activated = true;
                    self.set_topmost(false);
                }
            }
            WM_TIMER if wparam.0 == TIMER_WATCH => self.watch(),
            WM_TIMER => {
                // 見張り以外は 1 回限り
                self.kill_timer(wparam.0);
                match wparam.0 {
                    TIMER_GRACE => self.auto_hide(),
                    TIMER_RETRY => self.retry_foreground(),
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
                    self.hide("閉じる操作");
                    return Some(LRESULT(0));
                }
            }
            WM_DESTROY => {
                // SAFETY: メッセージループの終了
                unsafe { PostQuitMessage(0) };
            }
            WM_APP_SHOW => self.show("二重起動"),
            WM_POWERBROADCAST => {
                // スリープからの復帰直後はマウスフックが外されやすいので付け直す
                // (PBT_APMRESUMESUSPEND = 7, PBT_APMRESUMEAUTOMATIC = 0x12)
                if matches!(wparam.0, 7 | 0x12) {
                    crate::log::write("スリープから復帰");
                    desktop::rehook();
                }
                return None;
            }
            WM_APP_ICON => self.on_icons(),
            WM_APP_DESKTOP => self.show_at(wparam.0 as i32, lparam.0 as i32),
            crate::settings::WM_APP_SETTINGS => self.on_settings_closed(wparam.0 == 1),
            crate::settings::WM_APP_PREVIEW => {
                self.preview_look = crate::settings::preview_look();
                self.apply_look();
            }
            WM_MOVE => {
                // 壁紙ぼかしはウィンドウの裏にあたる部分を描くので、動いたら描き直す
                if self.backdrop.follows_position(&self.look()) {
                    self.invalidate();
                }
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                // 壁紙やディスプレイ構成の変更 (SPI_SETDESKWALLPAPER = 0x14)
                if msg == WM_DISPLAYCHANGE || wparam.0 == 0x14 {
                    self.backdrop.invalidate_wallpaper();
                    self.invalidate();
                }
                return None;
            }
            crate::edit::WM_APP_EDIT => self.on_edit_closed(wparam.0 == 1),
            WM_APP_SHELL => match ShellEvent::from_wparam(wparam.0) {
                Some(ShellEvent::Show) => self.show("トレイ"),
                Some(ShellEvent::Hotkey) => self.show("ホットキー"),
                Some(ShellEvent::Settings) => {
                    // 設定画面は本体に所有させるので、非表示なら先に復帰する
                    self.show("トレイの設定");
                    self.open_settings();
                }
                Some(ShellEvent::Exit) => self.exit(),
                None => {}
            },
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// 右クリックメニューの「ボタンの色」の最初のコマンド ID (色番号を足す)。
const COLOR_COMMAND: u32 = 100;

/// ボタンの色番号の表示名。
pub fn color_label(index: usize) -> String {
    if index == 0 {
        "標準".into()
    } else {
        format!("色 {index}")
    }
}

/// メニュー用の色見本 (角を丸めた四角、枠付き、32bpp のアルファ付き)。破棄は呼び出し側。
fn menu_swatch(c: Rgb, size: i32) -> Option<HBITMAP> {
    let size = size.max(8);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 大きさを指定した DIB を作り、確保された画素バッファにだけ書く
    unsafe {
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            return None;
        }
        let px = std::slice::from_raw_parts_mut(bits as *mut [u8; 4], (size * size) as usize);
        let last = size - 1;
        for y in 0..size {
            for x in 0..size {
                let edge = x == 0 || y == 0 || x == last || y == last;
                let corner = (x == 0 || x == last) && (y == 0 || y == last);
                // 事前乗算済みの BGRA。角は透明、外周は暗い枠
                px[(y * size + x) as usize] = if corner {
                    [0, 0, 0, 0]
                } else if edge {
                    [0x50, 0x50, 0x50, 0xFF]
                } else {
                    [c.2, c.1, c.0, 0xFF]
                };
            }
        }
        Some(bitmap)
    }
}

/// 自分のプロセスのウィンドウか (ダイアログ・メニュー・トレイのメニューなど)。
fn is_own_window(hwnd: HWND) -> bool {
    let mut pid = 0;
    // SAFETY: ウィンドウの所有プロセスの参照のみ
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

/// 診断ログ用のウィンドウ表記 (クラス名)。
fn window_label(hwnd: HWND) -> String {
    if hwnd.is_invalid() {
        return "なし".into();
    }
    window::class_name(hwnd)
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
            let _ = PostMessageW(
                Some(HWND(hwnd as *mut _)),
                msg,
                WPARAM(wparam),
                LPARAM(lparam),
            );
        }
    }
}

fn on_shell_event(event: ShellEvent) {
    post(WM_APP_SHELL, event.to_wparam(), 0);
}

fn on_desktop_double_click(x: i32, y: i32) {
    post(WM_APP_DESKTOP, x as usize, y as isize);
}
