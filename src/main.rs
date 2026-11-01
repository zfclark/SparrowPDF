//! 轻雀 PDF (SparrowPDF) — local-only PDF toolkit.
//!
//! PDF 的事，本地办完。

// Release builds must not open a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod model;
mod tools;
mod ui;

use eframe::egui;

const WINDOW_TITLE: &str = "轻雀 PDF";
const INITIAL_SIZE: [f32; 2] = [900.0, 650.0];

fn window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(app::ICON_PNG).ok()?;
    let rgba = img.to_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size([760.0, 520.0])
            .with_drag_and_drop(true)
            .with_icon(window_icon().expect("embedded launcher icon must decode")),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(app::SparrowApp::new(cc)))),
    )
}
