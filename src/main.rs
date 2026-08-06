#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod desktop_double_click;
mod hotkey;
mod icon;
mod single_instance;

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tray_icon::menu::MenuEvent;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuItem},
};
use windows::Win32::UI::WindowsAndMessaging::{GUI_INMOVESIZE, GUITHREADINFO, GetGUIThreadInfo};

const DEFAULT_WINDOW_WIDTH: f32 = 280.0;
const MIN_WINDOW_WIDTH: f32 = 200.0;
const MAX_WINDOW_WIDTH: f32 = 600.0;
const MAX_BACKGROUND_IMAGE_DIMENSION: u32 = 2000;
const CONFIG_PATH: &str = "LaunchPanel.json";
const LEGACY_CONFIG_PATH: &str = "launcher.json";

#[cfg(debug_assertions)]
const WINDOW_TITLE: &str = "LaunchPanel [DEBUG]";

#[cfg(not(debug_assertions))]
const WINDOW_TITLE: &str = "LaunchPanel";

#[derive(Serialize, Deserialize, Clone)]
struct LauncherItem {
    name: String,
    path: String,
}
#[derive(Serialize, Deserialize, Clone)]
struct WindowConfig {
    width: f32,
}

#[derive(Serialize, Deserialize, Clone)]
struct HotkeyConfig {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Config {
    settings: Settings,
    items: Vec<LauncherItem>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Settings {
    window: WindowConfig,
    hotkey: HotkeyConfig,
    #[serde(default)]
    background_image: String,
    #[serde(default)]
    item_button: ItemButtonConfig,
    #[serde(default)]
    desktop_double_click: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct ItemButtonConfig {
    background_color: [u8; 3],
    transparency: u8,
    text_color: [u8; 3],
    #[serde(default)]
    bold_text: bool,
}
impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_WINDOW_WIDTH,
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: true,
            win: false,
            key: "M".to_string(),
        }
    }
}

impl Default for ItemButtonConfig {
    fn default() -> Self {
        Self {
            background_color: [60, 60, 60],
            transparency: 0,
            text_color: [180, 180, 180],
            bold_text: false,
        }
    }
}

impl ItemButtonConfig {
    fn fill_color(&self) -> egui::Color32 {
        let [red, green, blue] = self.background_color;
        let transparency = self.transparency.min(100);
        let alpha = ((100 - u16::from(transparency)) * 255 / 100) as u8;

        egui::Color32::from_rgba_unmultiplied(red, green, blue, alpha)
    }

    fn text_color(&self) -> egui::Color32 {
        let [red, green, blue] = self.text_color;
        egui::Color32::from_rgb(red, green, blue)
    }
}

fn resized_window_width(
    current_window_width: f32,
    current_column_count: usize,
    old_column_width: f32,
    new_column_width: f32,
) -> Option<f32> {
    if old_column_width == new_column_width {
        return None;
    }

    Some(current_window_width + (new_column_width - old_column_width) * current_column_count as f32)
}

fn snapped_window_width(column_width: f32, column_count: usize) -> f32 {
    column_width * column_count.max(1) as f32
}

fn window_column_count(window_width: f32, column_width: f32, item_count: usize) -> usize {
    const WIDTH_BOUNDARY_TOLERANCE: f32 = 0.5;

    let column_count = ((window_width + WIDTH_BOUNDARY_TOLERANCE) / column_width).floor() as usize;
    column_count.max(1).min(item_count.max(1))
}

fn column_major_item_index(
    column_index: usize,
    row_index: usize,
    column_count: usize,
    item_count: usize,
) -> Option<usize> {
    if column_count == 0 || column_index >= column_count {
        return None;
    }

    let items_per_column = item_count / column_count;
    let columns_with_extra_item = item_count % column_count;
    let column_height = items_per_column + usize::from(column_index < columns_with_extra_item);

    if row_index >= column_height {
        return None;
    }

    let preceding_items =
        column_index * items_per_column + column_index.min(columns_with_extra_item);
    Some(preceding_items + row_index)
}

fn foreground_window_is_moving_or_resizing() -> bool {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };

    // SAFETY: `info` points to a valid writable GUITHREADINFO whose cbSize is initialized.
    unsafe { GetGUIThreadInfo(0, &mut info).is_ok() && info.flags.contains(GUI_INMOVESIZE) }
}

fn edit_rgb_color(ui: &mut egui::Ui, rgb: &mut [u8; 3]) -> egui::Response {
    let mut color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let response = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut color,
        egui::color_picker::Alpha::Opaque,
    );

    if response.changed() {
        let [red, green, blue, _] = color.to_array();
        *rgb = [red, green, blue];
    }

    response
}

fn shadowed_label(ui: &mut egui::Ui, text: impl Into<String>, small: bool) -> egui::Response {
    let text = text.into();
    let text_style = if small {
        egui::TextStyle::Small
    } else {
        egui::TextStyle::Body
    };
    let font_id = text_style.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text, font_id, egui::Color32::PLACEHOLDER);
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());

    ui.painter().galley(
        rect.min + egui::vec2(1.0, 1.0),
        galley.clone(),
        egui::Color32::BLACK,
    );
    ui.painter().galley(rect.min, galley, egui::Color32::WHITE);

    response
}

fn paint_hover_highlight(ui: &egui::Ui, rect: egui::Rect, corner_radius: u8) {
    ui.painter().rect_stroke(
        rect.expand(1.0),
        corner_radius,
        egui::Stroke::new(3.0_f32, egui::Color32::BLACK),
        egui::StrokeKind::Outside,
    );
    ui.painter().rect_stroke(
        rect,
        corner_radius,
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
}

fn rotate_point(point: egui::Vec2, angle: f32) -> egui::Vec2 {
    let (sin, cos) = angle.sin_cos();
    egui::vec2(point.x * cos - point.y * sin, point.x * sin + point.y * cos)
}

fn pin_button(ui: &mut egui::Ui, pinned: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let center = rect.center();
    let angle: f32 = if pinned {
        0.0
    } else {
        -std::f32::consts::FRAC_PI_4
    };
    let head_points = [
        egui::vec2(-5.5, -7.5),
        egui::vec2(5.5, -7.5),
        egui::vec2(3.5, -2.0),
        egui::vec2(-3.5, -2.0),
    ];
    let head_points: Vec<egui::Pos2> = head_points
        .into_iter()
        .map(|point| center + rotate_point(point, angle))
        .collect();
    let shaft_start = center + rotate_point(egui::vec2(0.0, -2.0), angle);
    let shaft_end = center + rotate_point(egui::vec2(0.0, 8.5), angle);

    if response.hovered() {
        let shadow_offset = egui::vec2(1.5, 1.5);
        let shadow_points = head_points
            .iter()
            .map(|point| *point + shadow_offset)
            .collect();
        ui.painter().add(egui::Shape::convex_polygon(
            shadow_points,
            if pinned {
                egui::Color32::BLACK
            } else {
                egui::Color32::TRANSPARENT
            },
            egui::Stroke::new(4.0_f32, egui::Color32::BLACK),
        ));
        ui.painter().line_segment(
            [shaft_start + shadow_offset, shaft_end + shadow_offset],
            egui::Stroke::new(4.0_f32, egui::Color32::BLACK),
        );
    }

    ui.painter().add(egui::Shape::convex_polygon(
        head_points,
        if pinned {
            egui::Color32::WHITE
        } else {
            egui::Color32::TRANSPARENT
        },
        egui::Stroke::new(
            if response.hovered() { 2.5_f32 } else { 1.5_f32 },
            if response.hovered() {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            },
        ),
    ));
    ui.painter().line_segment(
        [shaft_start, shaft_end],
        egui::Stroke::new(
            if pinned { 2.5_f32 } else { 1.5_f32 },
            if response.hovered() {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            },
        ),
    );

    response.on_hover_text(if pinned {
        "ピン留め解除"
    } else {
        "ピン留め"
    })
}

fn load_tray_icon() -> Icon {
    let png_bytes = include_bytes!("../LaunchPanel.png");

    let image = image::load_from_memory(png_bytes)
        .expect("PNG読込失敗")
        .into_rgba8();

    let (width, height) = image.dimensions();

    Icon::from_rgba(image.into_raw(), width, height).expect("トレイアイコン生成失敗")
}

fn load_fallback_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(include_bytes!("../LaunchPanel.png"))
        .expect("フォールバックアイコン読込失敗")
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());

    ctx.load_texture(
        "launcher-item-fallback",
        color_image,
        egui::TextureOptions::LINEAR,
    )
}

fn load_item_texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let image = icon::load(path)?;
    let color_image =
        egui::ColorImage::from_rgba_premultiplied([image.width, image.height], &image.rgba);

    Some(ctx.load_texture(
        format!("launcher-item-icon:{path}"),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn load_background_texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    if path.trim().is_empty() {
        return None;
    }

    if background_image_size_error(path).is_some() {
        return None;
    }

    let image = image::open(path).ok()?.into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());

    Some(ctx.load_texture(
        format!("background-image:{path}"),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn background_image_size_error(path: &str) -> Option<String> {
    if path.trim().is_empty() {
        return None;
    }

    let Ok((width, height)) = image::image_dimensions(path) else {
        return None;
    };

    background_image_dimensions_error(width, height)
}

fn background_image_dimensions_error(width: u32, height: u32) -> Option<String> {
    if width >= MAX_BACKGROUND_IMAGE_DIMENSION || height >= MAX_BACKGROUND_IMAGE_DIMENSION {
        Some(format!(
            "画像サイズが大き過ぎます（{width}×{height}px）。縦横とも2000px未満の画像を指定してください。"
        ))
    } else {
        None
    }
}

fn cover_uv(container_size: egui::Vec2, image_size: egui::Vec2) -> egui::Rect {
    let container_aspect = container_size.x / container_size.y;
    let image_aspect = image_size.x / image_size.y;

    if image_aspect > container_aspect {
        let visible_width = container_aspect / image_aspect;
        let margin = (1.0 - visible_width) / 2.0;
        egui::Rect::from_min_max(egui::pos2(margin, 0.0), egui::pos2(1.0 - margin, 1.0))
    } else {
        let visible_height = image_aspect / container_aspect;
        let margin = (1.0 - visible_height) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, margin), egui::pos2(1.0, 1.0 - margin))
    }
}

fn start_desktop_double_click_hook(
    show_requested: &Arc<AtomicBool>,
    repaint_ctx: &egui::Context,
) -> Option<desktop_double_click::DesktopDoubleClickHook> {
    let show_requested = show_requested.clone();
    let repaint_ctx = repaint_ctx.clone();
    desktop_double_click::DesktopDoubleClickHook::start(move || {
        show_requested.store(true, Ordering::SeqCst);
        repaint_ctx.request_repaint();
    })
    .inspect_err(|_error| {
        #[cfg(debug_assertions)]
        eprintln!("{_error}");
    })
    .ok()
}

fn main() -> eframe::Result {
    let single_instance = match single_instance::acquire().expect("単一起動の初期化に失敗しました")
    {
        single_instance::Acquisition::Primary(instance) => instance,
        single_instance::Acquisition::Existing => return Ok(()),
    };

    let tray_menu = Menu::new();

    let show_item = MenuItem::new("表示", true, None);
    let settings_item = MenuItem::new("設定", true, None);
    let quit_item = MenuItem::new("終了", true, None);
    let show_id = show_item.id().clone();
    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();

    tray_menu.append(&show_item).unwrap();
    tray_menu.append(&settings_item).unwrap();
    tray_menu.append(&quit_item).unwrap();

    let show_requested = Arc::new(AtomicBool::new(false));
    let settings_requested = Arc::new(AtomicBool::new(false));
    let settings_requested_for_thread = settings_requested.clone();

    fn load_window_icon() -> egui::IconData {
        let png_bytes = include_bytes!("../LaunchPanel.png");

        let image = image::load_from_memory(png_bytes)
            .expect("PNG読込失敗")
            .into_rgba8();

        let (width, height) = image.dimensions();

        egui::IconData {
            rgba: image.into_raw(),
            width,
            height,
        }
    }

    let _tray_icon = TrayIconBuilder::new()
        .with_tooltip("LaunchPanel")
        .with_icon(load_tray_icon())
        .with_menu(Box::new(tray_menu))
        .build()
        .expect("トレイアイコン作成失敗");

    let config = load_config();

    let hk = config.settings.hotkey.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.settings.window.width, 600.0])
            .with_min_inner_size([200.0, 200.0])
            .with_resizable(true)
            .with_maximize_button(false)
            .with_minimize_button(true)
            .with_close_button(true)
            .with_taskbar(false)
            .with_icon(load_window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();

            fonts.font_data.insert(
                "meiryo".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(
                    std::fs::read("C:/Windows/Fonts/meiryo.ttc").expect("Meiryo 読込失敗"),
                )),
            );

            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "meiryo".to_owned());

            fonts.font_data.insert(
                "meiryo_bold".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(
                    std::fs::read("C:/Windows/Fonts/meiryob.ttc").expect("Meiryo Bold 読込失敗"),
                )),
            );
            fonts.families.insert(
                egui::FontFamily::Name("meiryo_bold".into()),
                vec!["meiryo_bold".to_owned()],
            );

            cc.egui_ctx.set_fonts(fonts);

            let show_requested_for_thread = show_requested.clone();
            let repaint_ctx = cc.egui_ctx.clone();

            let show_requested_for_single_instance = show_requested.clone();
            let repaint_ctx_for_single_instance = cc.egui_ctx.clone();

            single_instance.listen(move || {
                show_requested_for_single_instance.store(true, Ordering::SeqCst);
                repaint_ctx_for_single_instance.request_repaint();
            });

            let show_requested_for_hotkey = show_requested.clone();
            let repaint_ctx_for_hotkey = cc.egui_ctx.clone();

            let hk_for_thread = hk.clone();

            std::thread::spawn(move || {
                hotkey::run(
                    hk_for_thread.ctrl,
                    hk_for_thread.alt,
                    hk_for_thread.shift,
                    hk_for_thread.win,
                    &hk_for_thread.key,
                    move || {
                        show_requested_for_hotkey.store(true, Ordering::SeqCst);
                        repaint_ctx_for_hotkey.request_repaint();
                    },
                );
            });

            std::thread::spawn(move || {
                let receiver = MenuEvent::receiver();

                loop {
                    if let Ok(event) = receiver.recv() {
                        if event.id == show_id {
                            show_requested_for_thread.store(true, Ordering::SeqCst);
                            repaint_ctx.request_repaint();
                        }

                        if event.id == quit_id {
                            std::process::exit(0);
                        } else if event.id == settings_id {
                            settings_requested_for_thread.store(true, Ordering::SeqCst);
                            repaint_ctx.request_repaint();
                        }
                    }
                }
            });

            let background_texture =
                load_background_texture(&cc.egui_ctx, &config.settings.background_image);

            let desktop_double_click_hook = if config.settings.desktop_double_click {
                start_desktop_double_click_hook(&show_requested, &cc.egui_ctx)
            } else {
                None
            };

            Ok(Box::new(MyApp {
                config: config.clone(),

                icon_cache: HashMap::new(),
                fallback_icon: load_fallback_texture(&cc.egui_ctx),
                background_texture,

                show_add_window: false,

                new_name: String::new(),
                new_path: String::new(),

                show_edit_window: false,

                edit_index: 0,

                edit_name: String::new(),
                edit_path: String::new(),

                show_delete_window: false,

                delete_index: 0,

                show_requested: show_requested.clone(),
                settings_requested: settings_requested.clone(),

                window_hidden: false,
                window_pinned: false,

                suppress_auto_hide_until_focused: true,

                show_settings_window: false,
                settings_edit: config.settings.clone(),
                settings_error: None,
                desktop_double_click_hook,

                last_observed_window_width: None,
                user_resize_active: false,
                programmatic_resize_target: None,
            }))
        }),
    )
}

struct MyApp {
    config: Config,

    icon_cache: HashMap<String, egui::TextureHandle>,
    fallback_icon: egui::TextureHandle,
    background_texture: Option<egui::TextureHandle>,

    show_add_window: bool,

    new_name: String,
    new_path: String,

    show_edit_window: bool,

    edit_index: usize,

    edit_name: String,
    edit_path: String,

    show_delete_window: bool,

    delete_index: usize,

    show_requested: Arc<AtomicBool>,
    settings_requested: Arc<AtomicBool>, // ←追加

    window_hidden: bool,
    window_pinned: bool,

    show_settings_window: bool,
    settings_edit: Settings, // ←追加
    settings_error: Option<String>,
    desktop_double_click_hook: Option<desktop_double_click::DesktopDoubleClickHook>,

    suppress_auto_hide_until_focused: bool,

    last_observed_window_width: Option<f32>,
    user_resize_active: bool,
    programmatic_resize_target: Option<f32>,
}

impl MyApp {
    fn refresh_icon_cache(&mut self, ctx: &egui::Context) {
        let paths: Vec<String> = self
            .config
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();

        self.icon_cache.retain(|path, _| paths.contains(path));

        for path in paths {
            if self.icon_cache.contains_key(&path) {
                continue;
            }

            let texture =
                load_item_texture(ctx, &path).unwrap_or_else(|| self.fallback_icon.clone());
            self.icon_cache.insert(path, texture);
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_icon_cache(ctx);

        let showing = self.show_requested.swap(false, Ordering::SeqCst);

        let window_width = ctx
            .input(|i| i.viewport().inner_rect)
            .map(|rect| rect.width())
            .unwrap_or(self.config.settings.window.width);
        let window_height = ctx
            .input(|i| i.viewport().inner_rect)
            .map(|rect| rect.height())
            .unwrap_or(600.0);

        let column_width = self.config.settings.window.width;
        let column_count = window_column_count(window_width, column_width, self.config.items.len());

        let focused = ctx.input(|i| i.focused);

        // Viewport focus can still contain the pre-restore value in the frame that consumes a
        // show request. Keep auto-hide suppressed until a later frame confirms real focus.
        if focused && !showing {
            self.suppress_auto_hide_until_focused = false;
        }

        let close_requested = ctx.input(|i| i.viewport().close_requested());

        let dialog_open = self.show_add_window
            || self.show_edit_window
            || self.show_settings_window
            || self.show_delete_window;

        let width_changed = self
            .last_observed_window_width
            .is_some_and(|previous| (window_width - previous).abs() > 0.5);
        let native_resize_active = foreground_window_is_moving_or_resizing();

        if let Some(target) = self.programmatic_resize_target {
            if width_changed || (window_width - target).abs() <= 0.5 {
                self.programmatic_resize_target = None;
            }
        } else if width_changed && native_resize_active && !dialog_open {
            self.user_resize_active = true;
        }

        if dialog_open {
            self.user_resize_active = false;
        } else if self.user_resize_active {
            if native_resize_active {
                ctx.request_repaint();
            } else {
                let target_width =
                    snapped_window_width(self.config.settings.window.width, column_count);
                if (window_width - target_width).abs() > 0.5 {
                    self.programmatic_resize_target = Some(target_width);
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        target_width,
                        window_height,
                    )));
                }
                self.user_resize_active = false;
            }
        }
        self.last_observed_window_width = Some(window_width);

        let auto_hide_requested = !focused
            && !dialog_open
            && !self.window_pinned
            && !self.suppress_auto_hide_until_focused;

        if self.settings_requested.swap(false, Ordering::SeqCst) {
            self.settings_edit = self.config.settings.clone();
            self.settings_error = None;
            self.show_settings_window = true;
        }

        if showing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();

            self.window_hidden = false;
            self.suppress_auto_hide_until_focused = true;
        } else if !self.window_hidden && close_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.window_hidden = true;
            self.window_pinned = false;
        } else if !self.window_hidden && auto_hide_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.window_hidden = true;
        }

        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
        let hovered_files = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let mut background_drop_handled = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(texture) = &self.background_texture {
                let rect = ui.max_rect();
                let image_size = texture.size_vec2();
                let uv = cover_uv(rect.size(), image_size);
                ui.painter()
                    .image(texture.id(), rect, uv, egui::Color32::WHITE);
            }

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if pin_button(ui, self.window_pinned).clicked() {
                        self.window_pinned = !self.window_pinned;
                    }

                    if ui
                        .add_sized([28.0, 28.0], egui::Button::new("⚙"))
                        .on_hover_text("設定")
                        .clicked()
                    {
                        self.settings_edit = self.config.settings.clone();
                        self.settings_error = None;
                        self.show_settings_window = true;
                    }

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        shadowed_label(
                            ui,
                            format!("{window_width:.0}×{window_height:.0} {column_count}列"),
                            true,
                        );
                    });
                },
            );

            // 1列あたりの行数
            let row_count = self.config.items.len().div_ceil(column_count);
            let item_button_fill = self.config.settings.item_button.fill_color();
            let item_button_text_color = self.config.settings.item_button.text_color();

            let mut item_list_frame = egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(10, 0))
                .begin(ui);
            item_list_frame.content_ui.columns(column_count, |columns| {
                for (column_index, column_ui) in columns.iter_mut().enumerate() {
                    for row_index in 0..row_count {
                        // 列の外形を先に決め、JSONの順番で左列から縦に埋める
                        let Some(index) = column_major_item_index(
                            column_index,
                            row_index,
                            column_count,
                            self.config.items.len(),
                        ) else {
                            continue;
                        };

                        let Some(item) = self.config.items.get(index) else {
                            continue;
                        };

                        let texture = self
                            .icon_cache
                            .get(&item.path)
                            .unwrap_or(&self.fallback_icon);
                        let image =
                            egui::Image::new(texture).fit_to_exact_size(egui::vec2(20.0, 20.0));
                        let mut text =
                            egui::RichText::new(item.name.as_str()).color(item_button_text_color);
                        if self.config.settings.item_button.bold_text {
                            text = text.family(egui::FontFamily::Name("meiryo_bold".into()));
                        }

                        let response = column_ui.add_sized(
                            [column_ui.available_width(), 36.0],
                            egui::Button::new((image, text, egui::Atom::grow()))
                                .fill(item_button_fill),
                        );

                        if response.hovered() {
                            paint_hover_highlight(column_ui, response.rect, 4);
                        }

                        if response.clicked() {
                            let _ = open::that(&item.path);
                        }

                        response.context_menu(|ui| {
                            if ui.button("編集").clicked() {
                                self.edit_index = index;

                                self.edit_name = item.name.clone();
                                self.edit_path = item.path.clone();

                                self.show_edit_window = true;

                                ui.close();
                            }

                            if ui.button("削除").clicked() {
                                self.delete_index = index;
                                self.show_delete_window = true;

                                ui.close();
                            }

                            ui.separator();

                            if ui.button("上へ移動").clicked() {
                                move_up = Some(index);
                                ui.close();
                            }

                            if ui.button("下へ移動").clicked() {
                                move_down = Some(index);
                                ui.close();
                            }
                        });
                    }
                }
            });
            item_list_frame.end(ui);
            // ドラッグ＆ドロップの説明
            ui.add_space(10.0);
            ui.separator();

            shadowed_label(ui, "ドラッグ＆ドロップで追加できます", true);

            if let Some(index) = move_up
                && index > 0
            {
                self.config.items.swap(index, index - 1);
                save_config(&self.config);
            }

            if let Some(index) = move_down
                && index + 1 < self.config.items.len()
            {
                self.config.items.swap(index, index + 1);
                save_config(&self.config);
            }
        });

        if self.show_add_window {
            egui::Window::new("項目追加").show(ctx, |ui| {
                ui.label("名前");
                ui.text_edit_singleline(&mut self.new_name);

                ui.label("パス");
                ui.text_edit_singleline(&mut self.new_path);

                if ui.button("登録").clicked() && !self.new_path.trim().is_empty() {
                    let name = if self.new_name.trim().is_empty() {
                        generate_name(&self.new_path)
                    } else {
                        self.new_name.clone()
                    };

                    self.config.items.push(LauncherItem {
                        name,
                        path: self.new_path.clone(),
                    });

                    save_config(&self.config);

                    self.new_name.clear();
                    self.new_path.clear();

                    self.show_add_window = false;
                }
                if ui.button("キャンセル").clicked() {
                    self.new_name.clear();
                    self.new_path.clear();

                    self.show_add_window = false;
                }
            });
        }
        if self.show_edit_window {
            egui::Window::new("項目編集").show(ctx, |ui| {
                ui.label("名前");
                ui.text_edit_singleline(&mut self.edit_name);

                ui.label("パス");
                ui.text_edit_singleline(&mut self.edit_path);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("保存").clicked() {
                        let name = if self.edit_name.trim().is_empty() {
                            generate_name(&self.edit_path)
                        } else {
                            self.edit_name.clone()
                        };

                        self.config.items[self.edit_index].name = name;

                        self.config.items[self.edit_index].path = self.edit_path.clone();

                        save_config(&self.config);

                        self.show_edit_window = false;
                    }

                    if ui.button("キャンセル").clicked() {
                        self.show_edit_window = false;
                    }
                });
            });
        }
        if self.show_delete_window {
            egui::Window::new("削除確認").show(ctx, |ui| {
                ui.label(format!(
                    "『{}』を削除しますか？",
                    self.config.items[self.delete_index].name
                ));

                ui.separator();

                ui.label(&self.config.items[self.delete_index].name);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("削除").clicked() {
                        self.config.items.remove(self.delete_index);

                        save_config(&self.config);

                        self.show_delete_window = false;
                    }

                    if ui.button("キャンセル").clicked() {
                        self.show_delete_window = false;
                    }
                });
            });
        }
        if self.show_settings_window {
            egui::Window::new("設定")
                .collapsible(false)
                .resizable(true)
                .default_size(egui::vec2(200.0, 360.0))
                .show(ctx, |ui| {
                    ui.set_min_width(176.0);
                    ui.set_max_width(176.0);
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            egui::CollapsingHeader::new("ウィンドウ")
                                .default_open(true)
                                .show(ui, |ui| {
                                    egui::Grid::new("settings_window_grid")
                                        .num_columns(2)
                                        .spacing([8.0, 8.0])
                                        .show(ui, |ui| {
                                            ui.label("1列の幅 (px)");
                                            ui.add(
                                                egui::DragValue::new(
                                                    &mut self.settings_edit.window.width,
                                                )
                                                .range(MIN_WINDOW_WIDTH..=MAX_WINDOW_WIDTH)
                                                .speed(1.0),
                                            );
                                            ui.end_row();
                                        });
                                });

                            egui::CollapsingHeader::new("背景")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.label("画像パス");
                                    let background_path = ui.text_edit_singleline(
                                        &mut self.settings_edit.background_image,
                                    );
                                    if background_path.changed() {
                                        self.settings_error = None;
                                    }
                                    ui.small("画像ファイルをこの欄へドラッグ＆ドロップできます");
                                    if hovered_files {
                                        background_path.highlight();
                                    }

                                    if let Some(path) =
                                        dropped_files.iter().find_map(|file| file.path.as_ref())
                                    {
                                        background_drop_handled = true;
                                        let path = path.to_string_lossy().to_string();
                                        if let Some(error) = background_image_size_error(&path) {
                                            self.settings_error = Some(error);
                                        } else {
                                            self.settings_edit.background_image = path;
                                            self.settings_error = None;
                                        }
                                    }
                                });

                            egui::CollapsingHeader::new("ボタン")
                                .default_open(false)
                                .show(ui, |ui| {
                                    egui::Grid::new("settings_item_button_grid")
                                        .num_columns(2)
                                        .spacing([8.0, 8.0])
                                        .show(ui, |ui| {
                                            ui.label("背景色");
                                            edit_rgb_color(
                                                ui,
                                                &mut self
                                                    .settings_edit
                                                    .item_button
                                                    .background_color,
                                            );
                                            ui.end_row();

                                            ui.label("透過率");
                                            ui.add(
                                                egui::DragValue::new(
                                                    &mut self
                                                        .settings_edit
                                                        .item_button
                                                        .transparency,
                                                )
                                                .range(0..=100)
                                                .suffix("%"),
                                            );
                                            ui.end_row();

                                            ui.label("テキスト色");
                                            edit_rgb_color(
                                                ui,
                                                &mut self.settings_edit.item_button.text_color,
                                            );
                                            ui.end_row();

                                            ui.label("文字");
                                            ui.checkbox(
                                                &mut self.settings_edit.item_button.bold_text,
                                                "太字にする",
                                            );
                                            ui.end_row();
                                        });

                                    if ui.button("デフォルトに戻す").clicked() {
                                        self.settings_edit.item_button =
                                            ItemButtonConfig::default();
                                    }

                                    let preview_fill = self.settings_edit.item_button.fill_color();
                                    let preview_text_color =
                                        self.settings_edit.item_button.text_color();
                                    let mut preview_text =
                                        egui::RichText::new("プレビュー").color(preview_text_color);
                                    if self.settings_edit.item_button.bold_text {
                                        preview_text = preview_text
                                            .family(egui::FontFamily::Name("meiryo_bold".into()));
                                    }
                                    let response = ui.add_sized(
                                        [ui.available_width(), 36.0],
                                        egui::Button::new(preview_text).fill(preview_fill),
                                    );
                                    if response.hovered() {
                                        paint_hover_highlight(ui, response.rect, 4);
                                    }
                                });

                            egui::CollapsingHeader::new("ホットキー")
                                .default_open(false)
                                .show(ui, |ui| {
                                    egui::Grid::new("settings_hotkey_grid")
                                        .num_columns(2)
                                        .spacing([8.0, 8.0])
                                        .show(ui, |ui| {
                                            ui.checkbox(
                                                &mut self.settings_edit.hotkey.ctrl,
                                                "Ctrl",
                                            );
                                            ui.end_row();
                                            ui.checkbox(&mut self.settings_edit.hotkey.alt, "Alt");
                                            ui.end_row();
                                            ui.checkbox(
                                                &mut self.settings_edit.hotkey.shift,
                                                "Shift",
                                            );
                                            ui.end_row();
                                            ui.add_enabled(
                                                false,
                                                egui::Checkbox::new(
                                                    &mut self.settings_edit.hotkey.win,
                                                    "Win",
                                                ),
                                            );
                                            ui.end_row();
                                            ui.label("キー");
                                            egui::ComboBox::from_id_salt("hotkey_key")
                                                .selected_text(&self.settings_edit.hotkey.key)
                                                .show_ui(ui, |ui| {
                                                    for c in 'A'..='Z' {
                                                        let s = c.to_string();
                                                        ui.selectable_value(
                                                            &mut self.settings_edit.hotkey.key,
                                                            s.clone(),
                                                            s,
                                                        );
                                                    }
                                                });
                                        });
                                });

                            egui::CollapsingHeader::new("実験的機能")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.checkbox(
                                        &mut self.settings_edit.desktop_double_click,
                                        "デスクトップ空白のダブルクリックで表示",
                                    );
                                    ui.small(
                                        "Windows更新やExplorerの構成によって、動作しない可能性があります。",
                                    );
                                });
                        });

                    if let Some(error) = &self.settings_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.show_settings_window = false;
                        }
                        if ui.button("OK").clicked() {
                            if let Some(error) =
                                background_image_size_error(&self.settings_edit.background_image)
                            {
                                self.settings_error = Some(error);
                            } else {
                                let resized_width = resized_window_width(
                                    window_width,
                                    column_count,
                                    self.config.settings.window.width,
                                    self.settings_edit.window.width,
                                );
                                let desktop_double_click_changed = self
                                    .config
                                    .settings
                                    .desktop_double_click
                                    != self.settings_edit.desktop_double_click;
                                self.config.settings = self.settings_edit.clone();
                                self.background_texture = load_background_texture(
                                    ctx,
                                    &self.config.settings.background_image,
                                );
                                save_config(&self.config);
                                if desktop_double_click_changed {
                                    self.desktop_double_click_hook = if self
                                        .config
                                        .settings
                                        .desktop_double_click
                                    {
                                        start_desktop_double_click_hook(
                                            &self.show_requested,
                                            ctx,
                                        )
                                    } else {
                                        None
                                    };
                                }
                                if let Some(resized_width) = resized_width {
                                    self.programmatic_resize_target = Some(resized_width);
                                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                                        egui::vec2(resized_width, window_height),
                                    ));
                                }
                                self.show_settings_window = false;
                            }
                        }
                    });
                });
        }

        if !background_drop_handled {
            let mut changed = false;

            for file in dropped_files {
                if let Some(path) = file.path {
                    let path_str = path.to_string_lossy().to_string();

                    if self.config.items.iter().any(|i| i.path == path_str) {
                        continue;
                    }

                    let name = generate_name(&path_str);

                    self.config.items.push(LauncherItem {
                        name,
                        path: path_str,
                    });

                    changed = true;
                }
            }
            if changed {
                save_config(&self.config);
            }
        }
    }
}
fn load_config() -> Config {
    let path = if Path::new(CONFIG_PATH).exists() {
        CONFIG_PATH
    } else if Path::new(LEGACY_CONFIG_PATH).exists() {
        LEGACY_CONFIG_PATH
    } else {
        let config = Config::default();
        save_config(&config);
        return config;
    };
    let migrating_legacy_config = path == LEGACY_CONFIG_PATH;

    let json = fs::read_to_string(path).unwrap_or_else(|error| panic!("{path} 読込失敗: {error}"));

    // 新フォーマット
    if let Ok(config) = serde_json::from_str::<Config>(&json) {
        if migrating_legacy_config {
            save_config(&config);
        }
        return config;
    }

    // 旧フォーマット（Vec<LauncherItem>）
    if let Ok(items) = serde_json::from_str::<Vec<LauncherItem>>(&json) {
        let config = Config {
            settings: Settings::default(),
            items,
        };

        save_config(&config);

        return config;
    }

    Config::default()
}
fn save_config(config: &Config) {
    let json = serde_json::to_string_pretty(config).expect("JSON変換失敗");

    std::fs::write(CONFIG_PATH, json)
        .unwrap_or_else(|error| panic!("{CONFIG_PATH} 保存失敗: {error}"));
}
fn generate_name(path: &str) -> String {
    // URL
    if path.starts_with("http://") || path.starts_with("https://") {
        let without_protocol = path.replace("https://", "").replace("http://", "");

        return without_protocol
            .split('/')
            .next()
            .unwrap_or(path)
            .to_string();
    }

    // ファイル・フォルダ
    if let Some(name) = Path::new(path).file_name() {
        return name.to_string_lossy().to_string();
    }

    // EXEなど
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        Config, background_image_dimensions_error, column_major_item_index, resized_window_width,
        snapped_window_width, window_column_count,
    };

    #[test]
    fn background_image_dimensions_must_be_below_2000_pixels() {
        assert!(background_image_dimensions_error(1999, 1999).is_none());
        assert!(background_image_dimensions_error(2000, 1999).is_some());
        assert!(background_image_dimensions_error(1999, 2000).is_some());
    }

    #[test]
    fn old_item_button_settings_default_to_regular_text() {
        let json = r#"{
            "settings": {
                "window": { "width": 280.0 },
                "hotkey": {
                    "ctrl": true,
                    "alt": true,
                    "shift": true,
                    "win": false,
                    "key": "M"
                },
                "item_button": {
                    "background_color": [60, 60, 60],
                    "transparency": 20,
                    "text_color": [180, 180, 180]
                }
            },
            "items": []
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.settings.item_button.bold_text);
    }

    #[test]
    fn unchanged_column_width_preserves_exact_window_size() {
        assert_eq!(resized_window_width(400.0, 2, 200.0, 200.0), None);
    }

    #[test]
    fn changed_column_width_preserves_column_count_and_existing_overhead() {
        assert_eq!(resized_window_width(421.0, 2, 200.0, 300.0), Some(621.0));
    }

    #[test]
    fn snapped_width_is_an_exact_multiple_of_the_column_width() {
        assert_eq!(snapped_window_width(200.0, 2), 400.0);
        assert_eq!(snapped_window_width(180.0, 3), 540.0);
    }

    #[test]
    fn snapped_width_always_keeps_at_least_one_column() {
        assert_eq!(snapped_window_width(200.0, 0), 200.0);
    }

    #[test]
    fn column_count_tolerates_subpixel_rounding_at_width_boundary() {
        assert_eq!(window_column_count(599.75, 200.0, 4), 3);
        assert_eq!(window_column_count(799.75, 200.0, 4), 4);
    }

    #[test]
    fn column_count_does_not_cross_a_real_width_boundary_early() {
        assert_eq!(window_column_count(599.0, 200.0, 4), 2);
    }

    #[test]
    fn four_items_in_three_columns_fill_down_each_column_in_json_order() {
        assert_eq!(column_major_item_index(0, 0, 3, 4), Some(0));
        assert_eq!(column_major_item_index(0, 1, 3, 4), Some(1));
        assert_eq!(column_major_item_index(1, 0, 3, 4), Some(2));
        assert_eq!(column_major_item_index(2, 0, 3, 4), Some(3));
        assert_eq!(column_major_item_index(1, 1, 3, 4), None);
    }

    #[test]
    fn five_items_in_three_columns_distribute_extra_items_to_left_columns() {
        assert_eq!(column_major_item_index(0, 0, 3, 5), Some(0));
        assert_eq!(column_major_item_index(0, 1, 3, 5), Some(1));
        assert_eq!(column_major_item_index(1, 0, 3, 5), Some(2));
        assert_eq!(column_major_item_index(1, 1, 3, 5), Some(3));
        assert_eq!(column_major_item_index(2, 0, 3, 5), Some(4));
        assert_eq!(column_major_item_index(2, 1, 3, 5), None);
    }

    #[test]
    fn evenly_divisible_items_fill_every_column_to_the_same_height() {
        assert_eq!(column_major_item_index(0, 0, 2, 4), Some(0));
        assert_eq!(column_major_item_index(0, 1, 2, 4), Some(1));
        assert_eq!(column_major_item_index(1, 0, 2, 4), Some(2));
        assert_eq!(column_major_item_index(1, 1, 2, 4), Some(3));
    }
}
