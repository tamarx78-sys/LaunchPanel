fn main() {
    let mut res = winres::WindowsResource::new();

    res.set_icon("appicon.ico");

    res.compile().expect("アイコン埋め込み失敗");
}
