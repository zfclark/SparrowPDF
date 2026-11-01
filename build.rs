//! Build script: embed the application icon (and manifest) into the Windows
//! executable.
//!
//! * Windows: `assets/app.ico` is compiled into the executable's resource
//!   section, and a manifest requests **administrator privileges** so saves
//!   into protected folders (Program Files, other drives' system paths, …)
//!   succeed out of the box. Build with `SPARROW_AS_INVOKER=1` to produce a
//!   non-elevated variant instead (drag-and-drop from Explorer keeps working
//!   in that variant; use it when per-user installs are preferred).
//! * macOS: the `.icns` is packaged into the `.app` bundle by the release
//!   workflow (see `.github/workflows/release.yml`); nothing to do here.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("FileDescription", "SparrowPDF");
        res.set("ProductName", "SparrowPDF");
        res.set_manifest(&manifest());
        // Only the application binary carries the manifest: linking it into
        // every target would force the test executables to request elevation
        // too, which would make `cargo test` unrunnable.
        res.compile_for("sparrowpdf")
            .expect("embed icon + manifest into the exe");
    }
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-env-changed=SPARROW_AS_INVOKER");
}

/// Application manifest.
///
/// Elevation is the default (`requireAdministrator`) because the tool writes
/// user-chosen output paths, which are frequently protected directories when
/// the app is installed machine-wide. `longPathAware` keeps >260-char output
/// paths working; `dpiAware` keeps the UI crisp on scaled displays.
fn manifest() -> String {
    let level = if std::env::var_os("SPARROW_AS_INVOKER").is_some() {
        "asInvoker"
    } else {
        "requireAdministrator"
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="SparrowPDF" version="1.0.0.0" processorArchitecture="*"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{level}" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
      <supportedOS Id="{{4f476546-9379-4f10-8daa-8a6a1ff2a5bd}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#
    )
}
