// exe へアイコンとアプリケーションマニフェスト (Per-Monitor V2 DPI、コモンコントロール v6) を埋め込む。
fn main() {
    println!("cargo:rerun-if-changed=assets/LaunchPanel.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/LaunchPanel.ico");
    res.set_manifest_file("assets/app.manifest");
    res.compile().expect("リソースの埋め込みに失敗しました");
}
