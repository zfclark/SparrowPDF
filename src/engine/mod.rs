//! Engine layer: PDF I/O, output validation, and background worker plumbing.
//!
//! This is the only module (besides `tools`) allowed to name `lopdf`,
//! `medpdf`, `medpdf_image`, `image` or `kamadak_exif` types.

pub mod logging;
pub mod pdf_io;
pub mod raster;
pub mod render;
pub mod ttf;
pub mod validate;
pub mod worker;
