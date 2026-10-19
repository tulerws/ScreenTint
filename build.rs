fn main() {
    slint_build::compile("ui/app.slint").unwrap();

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "ScreenTint")
            .set("FileDescription", "ScreenTint")
            .set("OriginalFilename", "ScreenTint.exe")
            .set("InternalName", "ScreenTint");
        res.compile().unwrap();
    }
}
