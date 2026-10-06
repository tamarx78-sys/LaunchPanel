//! OS 連携の低レベル処理。
//!
//! - `shell`: タスクトレイとグローバルホットキー (専用スレッドのメッセージループ)
//! - `icon`: シェルからのアイコン/サムネイル取得
//! - `shadow`: 枠なしウィンドウの影 (追従するレイヤードウィンドウ)
//! - `desktop`: デスクトップ空白ダブルクリックの検出
//! - `foreground`: フォアグラウンドロック下でも確実に前面化する
//! - `window`: 本体とダイアログで共通のウィンドウ操作

pub mod desktop;
pub mod foreground;
pub mod icon;
pub mod shadow;
pub mod shell;
pub mod wide;
pub mod window;
