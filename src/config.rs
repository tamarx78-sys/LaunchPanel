//! LaunchPanel.json の設定モデルと読み書き。
//!
//! 読込は寛容に行い、欠落・型違い・範囲外の値は項目単位で既定値または最小値へ補正する
//! (1項目の不正で全体を捨てない)。書き出しは LaunchPanel2 と同じキー・構造にする。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub const DEFAULT_COLUMN_WIDTH: f64 = 280.0;
pub const MIN_COLUMN_WIDTH: f64 = 200.0;
pub const MAX_COLUMN_WIDTH: f64 = 600.0;
pub const DEFAULT_HEIGHT: f64 = 600.0;
pub const MIN_INNER_SIZE: f64 = 200.0;
pub const DEFAULT_SHADOW_OPACITY: i32 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Debug, PartialEq)]
pub struct WindowSettings {
    /// 1列の基準幅 (実ウィンドウ幅ではない)
    pub width: f64,
    /// 実ウィンドウの内側幅・高さ (DIP)。未保存なら None
    pub inner_width: Option<f64>,
    pub inner_height: Option<f64>,
    pub shadow: bool,
    pub shadow_opacity: i32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: DEFAULT_COLUMN_WIDTH,
            inner_width: None,
            inner_height: None,
            shadow: true,
            shadow_opacity: DEFAULT_SHADOW_OPACITY,
        }
    }
}

impl WindowSettings {
    /// 起動時に使う内側幅。未保存の旧設定では列幅。
    pub fn effective_inner_width(&self) -> f64 {
        self.inner_width.unwrap_or(self.width.max(MIN_INNER_SIZE))
    }

    /// 起動時に使う内側高さ。未保存の旧設定では 600。
    pub fn effective_inner_height(&self) -> f64 {
        self.inner_height.unwrap_or(DEFAULT_HEIGHT)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HotkeySettings {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// データとして保持するのみ (現行 UI では変更不可)
    pub win: bool,
    pub key: char,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self { ctrl: true, alt: true, shift: true, win: false, key: 'M' }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemButtonSettings {
    pub background_color: Rgb,
    /// 0 = 不透明、100 = 完全透明
    pub transparency: i32,
    pub text_color: Rgb,
    pub bold_text: bool,
}

impl Default for ItemButtonSettings {
    fn default() -> Self {
        Self {
            background_color: Rgb(60, 60, 60),
            transparency: 0,
            text_color: Rgb(180, 180, 180),
            bold_text: false,
        }
    }
}

impl ItemButtonSettings {
    /// 透過率を 0.0～1.0 の不透明度へ変換する。
    pub fn background_alpha(&self) -> f32 {
        (100 - self.transparency) as f32 / 100.0
    }
}

pub const DEFAULT_BLUR: i32 = 40;
pub const DEFAULT_TINT: i32 = 35;

/// 背景画像を設定していない時の背景。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// 単色
    None,
    /// デスクトップの壁紙のウィンドウの裏にあたる部分をぼかして敷く (ぼかしの強さを調整できる)
    #[default]
    Wallpaper,
    /// Windows 標準のアクリル (裏のウィンドウも透けて見える。ぼかしの強さは OS が決める)
    Acrylic,
}

impl Backdrop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Wallpaper => "wallpaper",
            Self::Acrylic => "acrylic",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "wallpaper" => Some(Self::Wallpaper),
            "acrylic" => Some(Self::Acrylic),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub window: WindowSettings,
    pub hotkey: HotkeySettings,
    /// 空文字は「背景画像なし」
    pub background_image: String,
    pub backdrop: Backdrop,
    /// ぼかしの強さ (0～100)。壁紙ぼかしと背景画像に効く
    pub blur: i32,
    /// 背景に重ねる暗さ (0～100)
    pub tint: i32,
    pub item_button: ItemButtonSettings,
    pub desktop_double_click: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: WindowSettings::default(),
            hotkey: HotkeySettings::default(),
            background_image: String::new(),
            backdrop: Backdrop::default(),
            blur: DEFAULT_BLUR,
            tint: DEFAULT_TINT,
            item_button: ItemButtonSettings::default(),
            desktop_double_click: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Config {
    pub settings: Settings,
    pub items: Vec<Item>,
}

// ───────────── 読込 ─────────────

/// JSON 文字列を解析する。JSON として解釈できなければ None。
/// 2つ目の戻り値は、ルートがアイテム配列だけの旧形式だったか。
pub fn parse(json: &str) -> Option<(Config, bool)> {
    let root: Value = serde_json::from_str(json).ok()?;
    let mut config = Config::default();
    match root {
        Value::Array(items) => {
            config.items = read_items(&items);
            Some((config, true))
        }
        Value::Object(obj) => {
            if let Some(Value::Object(s)) = obj.get("settings") {
                read_settings(s, &mut config.settings);
            }
            if let Some(Value::Array(items)) = obj.get("items") {
                config.items = read_items(items);
            }
            Some((config, false))
        }
        _ => None,
    }
}

fn read_settings(node: &Map<String, Value>, s: &mut Settings) {
    if let Some(Value::Object(w)) = node.get("window") {
        if let Some(width) = number(w.get("width")) {
            s.window.width = width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
        }
        s.window.inner_width = number(w.get("inner_width")).map(|v| v.max(MIN_INNER_SIZE));
        s.window.inner_height = number(w.get("inner_height")).map(|v| v.max(MIN_INNER_SIZE));
        s.window.shadow = boolean(w.get("shadow")).unwrap_or(true);
        if let Some(o) = number(w.get("shadow_opacity")) {
            s.window.shadow_opacity = o.clamp(0.0, 100.0).round() as i32;
        }
    }
    if let Some(Value::Object(h)) = node.get("hotkey") {
        let d = &mut s.hotkey;
        d.ctrl = boolean(h.get("ctrl")).unwrap_or(d.ctrl);
        d.alt = boolean(h.get("alt")).unwrap_or(d.alt);
        d.shift = boolean(h.get("shift")).unwrap_or(d.shift);
        d.win = boolean(h.get("win")).unwrap_or(d.win);
        if let Some(key) = string(h.get("key")).and_then(valid_hotkey_key) {
            d.key = key;
        }
    }
    s.background_image = string(node.get("background_image")).unwrap_or_default().to_owned();
    s.backdrop = string(node.get("backdrop")).and_then(Backdrop::parse).unwrap_or_default();
    // 旧設定に項目が無い場合、背景画像があれば見た目を変えない (ぼかし・暗さなし)
    let has_image = !s.background_image.trim().is_empty();
    s.blur = percent(node.get("blur")).unwrap_or(if has_image { 0 } else { DEFAULT_BLUR });
    s.tint = percent(node.get("tint")).unwrap_or(if has_image { 0 } else { DEFAULT_TINT });
    if let Some(Value::Object(b)) = node.get("item_button") {
        let d = &mut s.item_button;
        d.background_color = rgb(b.get("background_color")).unwrap_or(d.background_color);
        if let Some(t) = number(b.get("transparency")) {
            d.transparency = t.clamp(0.0, 100.0).round() as i32;
        }
        d.text_color = rgb(b.get("text_color")).unwrap_or(d.text_color);
        d.bold_text = boolean(b.get("bold_text")).unwrap_or(false);
    }
    s.desktop_double_click = boolean(node.get("desktop_double_click")).unwrap_or(false);
}

fn read_items(array: &[Value]) -> Vec<Item> {
    array
        .iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let path = string(obj.get("path"))?;
            if path.trim().is_empty() {
                return None;
            }
            let name = string(obj.get("name")).filter(|n| !n.is_empty());
            Some(Item {
                name: name.map(str::to_owned).unwrap_or_else(|| crate::items::default_name(path)),
                path: path.to_owned(),
            })
        })
        .collect()
}

pub fn valid_hotkey_key(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?.to_ascii_uppercase();
    (chars.next().is_none() && c.is_ascii_uppercase()).then_some(c)
}

/// 有限の数値のみ受け付ける (文字列・NaN・無限大は None)。
fn number(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().filter(|d| d.is_finite())
}

/// 0～100 へ丸めた整数。数値でなければ None。
fn percent(v: Option<&Value>) -> Option<i32> {
    number(v).map(|n| n.clamp(0.0, 100.0).round() as i32)
}

fn boolean(v: Option<&Value>) -> Option<bool> {
    v?.as_bool()
}

fn string(v: Option<&Value>) -> Option<&str> {
    v?.as_str()
}

fn rgb(v: Option<&Value>) -> Option<Rgb> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut c = [0u8; 3];
    for (i, x) in a.iter().enumerate() {
        c[i] = number(Some(x))?.clamp(0.0, 255.0).round() as u8;
    }
    Some(Rgb(c[0], c[1], c[2]))
}

// ───────────── 書き出し ─────────────

pub fn serialize(config: &Config) -> String {
    let s = &config.settings;
    let mut window = Map::new();
    window.insert("width".into(), json!(s.window.width));
    if let Some(w) = s.window.inner_width {
        window.insert("inner_width".into(), json!(w));
    }
    if let Some(h) = s.window.inner_height {
        window.insert("inner_height".into(), json!(h));
    }
    window.insert("shadow".into(), json!(s.window.shadow));
    window.insert("shadow_opacity".into(), json!(s.window.shadow_opacity));

    let b = &s.item_button;
    let root = json!({
        "settings": {
            "window": window,
            "hotkey": {
                "ctrl": s.hotkey.ctrl,
                "alt": s.hotkey.alt,
                "shift": s.hotkey.shift,
                "win": s.hotkey.win,
                "key": s.hotkey.key.to_string(),
            },
            "background_image": s.background_image,
            "backdrop": s.backdrop.as_str(),
            "blur": s.blur,
            "tint": s.tint,
            "item_button": {
                "background_color": [b.background_color.0, b.background_color.1, b.background_color.2],
                "transparency": b.transparency,
                "text_color": [b.text_color.0, b.text_color.1, b.text_color.2],
                "bold_text": b.bold_text,
            },
            "desktop_double_click": s.desktop_double_click,
        },
        "items": config.items.iter().map(|i| json!({ "name": i.name, "path": i.path })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&root).unwrap_or_default()
}

// ───────────── ファイル ─────────────

pub const FILE_NAME: &str = "LaunchPanel.json";
pub const LEGACY_FILE_NAME: &str = "launcher.json";
pub const INVALID_BACKUP_SUFFIX: &str = ".invalid";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadSource {
    CreatedDefault,
    Current,
    MigratedArrayFormat,
    MigratedLauncherJson,
    RecoveredFromInvalid,
}

/// 指定フォルダー (アプリでは exe のあるフォルダー) 上の設定ファイルの読込・移行・保存。
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// LaunchPanel.json を最優先し、無ければ launcher.json から移行する。
    /// 既定値作成・移行・破損からの復旧時は、結果を直ちに現行形式で保存する。
    pub fn load(&self) -> (Config, LoadSource) {
        let current = self.path();
        let legacy = self.dir.join(LEGACY_FILE_NAME);
        let (config, source) = if current.exists() {
            match read(&current) {
                Some((c, true)) => (c, LoadSource::MigratedArrayFormat),
                Some((c, false)) => (c, LoadSource::Current),
                None => {
                    let backup = PathBuf::from(format!("{}{INVALID_BACKUP_SUFFIX}", current.display()));
                    if let Err(e) = std::fs::copy(&current, &backup) {
                        crate::log::write(&format!("不正な設定ファイルの退避に失敗しました: {e}"));
                    }
                    (Config::default(), LoadSource::RecoveredFromInvalid)
                }
            }
        } else if let Some((c, _)) = legacy.exists().then(|| read(&legacy)).flatten() {
            (c, LoadSource::MigratedLauncherJson)
        } else {
            (Config::default(), LoadSource::CreatedDefault)
        };
        if source != LoadSource::Current {
            self.save(&config);
        }
        (config, source)
    }

    /// 一時ファイルへ書いてから置き換える。失敗はログへ記録して false を返す。
    pub fn save(&self, config: &Config) -> bool {
        let path = self.path();
        let temp = path.with_extension("json.tmp");
        let result = std::fs::write(&temp, serialize(config)).and_then(|_| std::fs::rename(&temp, &path));
        if let Err(e) = &result {
            crate::log::write(&format!("設定の保存に失敗しました: {}: {e}", path.display()));
            let _ = std::fs::remove_file(&temp);
        }
        result.is_ok()
    }
}

fn read(path: &Path) -> Option<(Config, bool)> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(text.trim_start_matches('\u{feff}')),
        Err(e) => {
            crate::log::write(&format!("設定の読込に失敗しました: {}: {e}", path.display()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_all_fields() {
        let mut c = Config::default();
        c.settings.window = WindowSettings {
            width: 300.0,
            inner_width: Some(600.0),
            inner_height: Some(450.0),
            shadow: false,
            shadow_opacity: 75,
        };
        c.settings.hotkey = HotkeySettings { ctrl: false, alt: true, shift: false, win: true, key: 'Q' };
        c.settings.background_image = r"C:\bg.png".into();
        c.settings.item_button =
            ItemButtonSettings { background_color: Rgb(1, 2, 3), transparency: 40, text_color: Rgb(250, 251, 252), bold_text: true };
        c.settings.desktop_double_click = true;
        c.items.push(Item { name: "Example".into(), path: r"C:\Path\To\Example.exe".into() });
        let (parsed, legacy) = parse(&serialize(&c)).unwrap();
        assert!(!legacy);
        assert_eq!(parsed, c);
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let (c, _) = parse(
            r#"{"settings":{"window":{"width":250.0},"hotkey":{"ctrl":true,"alt":false,"shift":true,"win":false,"key":"K"}},
               "items":[{"name":"N","path":"P"}]}"#,
        )
        .unwrap();
        assert_eq!(c.settings.window.width, 250.0);
        assert_eq!(c.settings.window.effective_inner_width(), 250.0);
        assert_eq!(c.settings.window.effective_inner_height(), 600.0);
        assert!(c.settings.window.shadow);
        assert_eq!(c.settings.window.shadow_opacity, DEFAULT_SHADOW_OPACITY);
        assert!(!c.settings.hotkey.alt);
        assert_eq!(c.settings.hotkey.key, 'K');
        assert_eq!(c.settings.item_button, ItemButtonSettings::default());
        assert!(!c.settings.desktop_double_click);
        assert_eq!(c.settings.backdrop, Backdrop::Wallpaper);
        assert_eq!((c.settings.blur, c.settings.tint), (DEFAULT_BLUR, DEFAULT_TINT));
        assert_eq!(c.items.len(), 1);
    }

    #[test]
    fn invalid_values_are_corrected() {
        let (c, _) = parse(
            r#"{"settings":{"window":{"width":"wide","inner_width":50,"inner_height":"tall","shadow_opacity":150},
               "hotkey":{"key":"F1"},
               "item_button":{"background_color":[300,-5,"x"],"transparency":150,"text_color":[10,20]}},
               "items":[{"name":"x"},{"path":"C:\\a.exe"},5]}"#,
        )
        .unwrap();
        assert_eq!(c.settings.window.width, 280.0);
        assert_eq!(c.settings.window.inner_width, Some(200.0));
        assert_eq!(c.settings.window.inner_height, None);
        assert_eq!(c.settings.window.shadow_opacity, 100);
        assert_eq!(c.settings.hotkey.key, 'M');
        assert_eq!(c.settings.item_button.background_color, Rgb(60, 60, 60));
        assert_eq!(c.settings.item_button.transparency, 100);
        assert_eq!(c.items, vec![Item { name: "a.exe".into(), path: r"C:\a.exe".into() }]);
    }

    #[test]
    fn backdrop_values() {
        // 背景画像のある旧設定は、ぼかし・暗さなしで今までと同じ見た目
        let (c, _) = parse(r#"{"settings":{"background_image":"C:\bg.png"}}"#).unwrap();
        assert_eq!((c.settings.blur, c.settings.tint), (0, 0));
        let (c, _) = parse(r#"{"settings":{"backdrop":"acrylic","blur":150,"tint":-3}}"#).unwrap();
        assert_eq!(c.settings.backdrop, Backdrop::Acrylic);
        assert_eq!((c.settings.blur, c.settings.tint), (100, 0));
        let (c, _) = parse(r#"{"settings":{"backdrop":"glass","blur":"x"}}"#).unwrap();
        assert_eq!(c.settings.backdrop, Backdrop::Wallpaper);
        assert_eq!(c.settings.blur, DEFAULT_BLUR);
    }

    #[test]
    fn column_width_is_clamped() {
        assert_eq!(parse(r#"{"settings":{"window":{"width":9999}}}"#).unwrap().0.settings.window.width, 600.0);
        assert_eq!(parse(r#"{"settings":{"window":{"width":10}}}"#).unwrap().0.settings.window.width, 200.0);
    }

    #[test]
    fn legacy_array_root() {
        let (c, legacy) = parse(r#"[{"name":"A","path":"a"},{"name":"B","path":"b"}]"#).unwrap();
        assert!(legacy);
        assert_eq!(c.items.len(), 2);
    }

    #[test]
    fn unparseable_is_none() {
        assert!(parse("{not json").is_none());
        assert!(parse("42").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn reads_launchpanel2_output() {
        // LaunchPanel2 (C#) が書いた形式をそのまま読める
        let json = r#"{
  "settings": {
    "window": { "width": 280, "inner_width": 200, "inner_height": 436.67, "shadow": true, "shadow_opacity": 80 },
    "hotkey": { "ctrl": true, "alt": true, "shift": true, "win": false, "key": "K" },
    "background_image": "J:\\bg.png",
    "item_button": { "background_color": [ 60, 60, 60 ], "transparency": 0, "text_color": [ 180, 180, 180 ], "bold_text": false },
    "desktop_double_click": true
  },
  "items": [ { "name": "メモ帳", "path": "notepad" } ]
}"#;
        let (c, _) = parse(json).unwrap();
        assert_eq!(c.settings.window.inner_height, Some(436.67));
        assert_eq!(c.settings.window.shadow_opacity, 80);
        assert!(c.settings.desktop_double_click);
        assert_eq!(c.items[0].name, "メモ帳");
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lp3-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn store_first_run_and_migrations() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        assert_eq!(store.load().1, LoadSource::CreatedDefault);
        assert!(store.path().exists());

        std::fs::remove_file(store.path()).unwrap();
        std::fs::write(dir.join(LEGACY_FILE_NAME), r#"[{"name":"Old","path":"old.exe"}]"#).unwrap();
        let (c, source) = store.load();
        assert_eq!(source, LoadSource::MigratedLauncherJson);
        assert_eq!(c.items[0].name, "Old");

        std::fs::write(store.path(), r#"{"items":[{"name":"New","path":"new.exe"}]}"#).unwrap();
        let (c, source) = store.load();
        assert_eq!(source, LoadSource::Current, "LaunchPanel.json が launcher.json より優先");
        assert_eq!(c.items[0].name, "New");

        std::fs::write(store.path(), "{broken").unwrap();
        let (c, source) = store.load();
        assert_eq!(source, LoadSource::RecoveredFromInvalid);
        assert!(c.items.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join(format!("{FILE_NAME}{INVALID_BACKUP_SUFFIX}"))).unwrap(), "{broken");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
