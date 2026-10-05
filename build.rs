// exe へアイコン、アプリケーションマニフェスト (Per-Monitor V2 DPI、コモンコントロール v6)、
// バージョン情報 (エクスプローラーのプロパティ > 詳細) を埋め込む。
fn main() {
    println!("cargo:rerun-if-changed=assets/LaunchPanel.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/LaunchPanel.ico");
    res.set_manifest_file("assets/app.manifest");
    // FileVersion / ProductVersion は Cargo.toml の version から自動で入る
    res.set("ProductName", "LaunchPanel");
    res.set("FileDescription", "LaunchPanel");
    res.set("OriginalFilename", "LaunchPanel.exe");
    res.set("LegalCopyright", "Copyright (c) 2026 Tamarx78-sys");
    res.compile().expect("リソースの埋め込みに失敗しました");
}
