//! Build script: embed the application icon into platform binaries.
//!
//! * Windows: `assets/app.ico` is compiled into the executable's resource
//!   section, so Explorer / taskbar / shortcuts show the app icon.
//! * macOS: the `.icns` is packaged into the `.app` bundle by the release
//!   workflow (see `.github/workflows/release.yml`); nothing to do here.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.compile().expect("embed assets/app.ico into the exe");
    }
    println!("cargo:rerun-if-changed=assets/app.ico");
}
