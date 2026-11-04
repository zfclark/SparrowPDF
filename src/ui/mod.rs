//! UI layer.
//!
//! Only `model`, `tools` (the `Tool` trait and option types) and `engine`
//! helpers with `ToolError`-shaped results are visible here. No `lopdf`,
//! `medpdf`, `medpdf_image`, `image` or `kamadak_exif` type is imported anywhere
//! under `ui/`.

pub mod about;
pub mod conflict;
pub mod panels;
pub mod prefs;
pub mod preview;
pub mod sidebar;
pub mod status;
pub mod theme;
