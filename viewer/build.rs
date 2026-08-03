use shadow_rs::ShadowBuilder;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/favicon.ico");
        res.compile().unwrap();
    }

    ShadowBuilder::builder().build().unwrap();
}
