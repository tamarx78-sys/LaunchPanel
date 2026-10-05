//! 診断用ログ。exe と同じフォルダーの LaunchPanel.log へ追記する (仕様 19.1)。

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const FILE_NAME: &str = "LaunchPanel.log";
const MAX_BYTES: u64 = 512 * 1024;
static GATE: Mutex<()> = Mutex::new(());

fn path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| crate::app_dir().join(FILE_NAME))
}

pub fn write(message: &str) {
    let _guard = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let path = path();
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    // ログが書けなくても動作は継続する
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {message}", timestamp());
    }
}

fn timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: 出力専用の呼び出し
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// 環境変数 LAUNCHPANEL_DEBUG=1 の時だけ書く診断ログ (復帰やフォーカスの判断の追跡用)。
pub fn debug(message: &str) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var("LAUNCHPANEL_DEBUG").is_ok_and(|v| v == "1")) {
        write(&format!("[debug] {message}"));
    }
}
