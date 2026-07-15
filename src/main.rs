#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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

const DEFAULT_WINDOW_WIDTH: f32 = 280.0;
const MIN_WINDOW_WIDTH: f32 = 200.0;
const MAX_WINDOW_WIDTH: f32 = 600.0;

#[cfg(debug_assertions)]
const WINDOW_TITLE: &str = "MiniLauncher [DEBUG]";

#[cfg(not(debug_assertions))]
const WINDOW_TITLE: &str = "MiniLauncher";

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

fn load_tray_icon() -> Icon {
    let png_bytes = include_bytes!("../appicon.png");

    let image = image::load_from_memory(png_bytes)
        .expect("PNG読込失敗")
        .into_rgba8();

    let (width, height) = image.dimensions();

    Icon::from_rgba(image.into_raw(), width, height).expect("トレイアイコン生成失敗")
}

fn load_fallback_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(include_bytes!("../appicon.png"))
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
        let png_bytes = include_bytes!("../appicon.png");

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
        .with_tooltip("MiniLauncher")
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

            Ok(Box::new(MyApp {
                config: config.clone(),

                icon_cache: HashMap::new(),
                fallback_icon: load_fallback_texture(&cc.egui_ctx),

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

                suppress_auto_hide_until_focused: true,

                show_settings_window: false,
                settings_edit: config.settings.clone(),
            }))
        }),
    )
}

struct MyApp {
    config: Config,

    icon_cache: HashMap<String, egui::TextureHandle>,
    fallback_icon: egui::TextureHandle,

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

    show_settings_window: bool,
    settings_edit: Settings, // ←追加

    suppress_auto_hide_until_focused: bool,
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

        let focused = ctx.input(|i| i.focused);

        if focused {
            self.suppress_auto_hide_until_focused = false;
        }

        let close_requested = ctx.input(|i| i.viewport().close_requested());

        let dialog_open = self.show_add_window
            || self.show_edit_window
            || self.show_settings_window
            || self.show_delete_window;

        let auto_hide_requested =
            !focused && !dialog_open && !self.suppress_auto_hide_until_focused;

        if self.settings_requested.swap(false, Ordering::SeqCst) {
            self.settings_edit = self.config.settings.clone();
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

            if !dialog_open {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                self.window_hidden = true;
            }
        } else if !self.window_hidden && auto_hide_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.window_hidden = true;
        }

        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MiniLauncher");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+").clicked() {
                        self.show_add_window = true;
                    }
                });
            });

            let column_width = self.config.settings.window.width;

            let column_count = (window_width / column_width).floor().max(1.0) as usize;

            // アイテム数より列数が多くならないようにする
            let column_count = column_count.min(self.config.items.len().max(1));

            // 1列あたりの行数
            let row_count = self.config.items.len().div_ceil(column_count);

            ui.columns(column_count, |columns| {
                for (column_index, column_ui) in columns.iter_mut().enumerate() {
                    for row_index in 0..row_count {
                        // 縦方向に並べてから、次の列へ移る
                        let index = column_index * row_count + row_index;

                        let Some(item) = self.config.items.get(index) else {
                            continue;
                        };

                        let texture = self
                            .icon_cache
                            .get(&item.path)
                            .unwrap_or(&self.fallback_icon);
                        let image =
                            egui::Image::new(texture).fit_to_exact_size(egui::vec2(20.0, 20.0));

                        let response = column_ui.add_sized(
                            [column_ui.available_width(), 36.0],
                            egui::Button::new((image, item.name.as_str(), egui::Atom::grow())),
                        );

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
            // 一時的に表示
            ui.label(format!(
                "Width: {:.0}  Columns: {}",
                window_width, column_count
            ));
            // ドラッグ＆ドロップの説明
            ui.add_space(10.0);
            ui.separator();

            ui.small("ドラッグ＆ドロップで追加できます");

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

        // ドロップされたファイルの処理
        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());

        let mut changed = false;

        for file in dropped_files {
            if let Some(path) = file.path {
                let path_str = path.to_string_lossy().to_string();

                // 重複チェック
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
                .resizable(false)
                .default_size(egui::vec2(280.0, 120.0))
                .show(ctx, |ui| {
                    ui.heading("ウィンドウ");
                    egui::Grid::new("settings_window_grid")
                        .num_columns(2)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("1列の幅 (px)");

                            ui.add(
                                egui::DragValue::new(&mut self.settings_edit.window.width)
                                    .range(MIN_WINDOW_WIDTH..=MAX_WINDOW_WIDTH)
                                    .speed(1.0),
                            );

                            ui.end_row();
                        });
                    ui.separator();
                    ui.heading("ホットキー");
                    egui::Grid::new("settings_hotkey_grid")
                        .num_columns(2)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.checkbox(&mut self.settings_edit.hotkey.ctrl, "Ctrl");
                            ui.end_row();

                            ui.checkbox(&mut self.settings_edit.hotkey.alt, "Alt");
                            ui.end_row();

                            ui.checkbox(&mut self.settings_edit.hotkey.shift, "Shift");
                            ui.end_row();

                            ui.add_enabled(
                                false,
                                egui::Checkbox::new(&mut self.settings_edit.hotkey.win, "Win"),
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

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.show_settings_window = false;
                        }
                        if ui.button("OK").clicked() {
                            self.config.settings = self.settings_edit.clone();
                            save_config(&self.config);
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                                self.config.settings.window.width,
                                600.0,
                            )));
                            self.show_settings_window = false;
                        }
                    });
                });
        }
    }
}
fn load_config() -> Config {
    let path = "launcher.json";

    if !std::path::Path::new(path).exists() {
        let config = Config::default();
        save_config(&config);
        return config;
    }

    let json = fs::read_to_string(path).expect("launcher.json 読込失敗");

    // 新フォーマット
    if let Ok(config) = serde_json::from_str::<Config>(&json) {
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

    std::fs::write("launcher.json", json).expect("保存失敗");
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
