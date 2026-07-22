fn main() {
    let mut res = winres::WindowsResource::new();

    res.set_icon("LaunchPanel.ico");

    res.compile().expect("アイコン埋め込み失敗");
}
