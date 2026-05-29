fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("src/assets/app_logo.ico");
        res.compile().expect("failed to compile Windows resources");
    }
}
