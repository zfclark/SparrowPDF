//! SparrowPDF library facade — makes the internals reachable from `tests/`
//! and from `cargo test` without spinning up the GUI.
//!
//! The binary in `main.rs` declares the same modules; the two entry points
//! share one copy of the sources, so keep this list in sync with `main.rs`.

pub mod app;
pub mod engine;
pub mod model;
pub mod tools;
pub mod ui;
