//! Thumbnail page previews for the image→PDF tools.
//!
//! Shows what the produced pages will look like: one thumbnail per image, in
//! final page order. Decoding is budgeted (a few images per frame) so large
//! folders never stall the UI, and textures are cached per path for the whole
//! session. The area collapses / expands on demand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Sense, Ui, Vec2};

use crate::ui::theme;

/// Thumbnail box height; the width adapts to the image aspect ratio.
const THUMB_H: f32 = 96.0;
/// Thumbnail box width cap (portrait images never get wider than this).
const THUMB_W_MAX: f32 = 84.0;
/// How many new thumbnails may be decoded per frame.
const DECODE_BUDGET: usize = 3;
/// Re-scan folder contents every N frames while the preview is open, so
/// newly added images show up without a manual refresh.
const RESCAN_FRAMES: u64 = 120;

enum Thumb {
    Ready(egui::TextureHandle),
    Failed,
}

/// Per-session preview state, owned by the app shell.
pub struct PreviewState {
    /// Expanded / collapsed.
    pub open: bool,
    textures: BTreeMap<PathBuf, Thumb>,
    pending: Vec<PathBuf>,
    /// Cached folder listings for the folder tool, keyed by the folder list.
    folders_cache: Option<(Vec<PathBuf>, Vec<(String, Vec<PathBuf>)>)>,
    frame: u64,
}

impl PreviewState {
    pub fn new() -> Self {
        Self {
            open: true,
            textures: BTreeMap::new(),
            pending: Vec::new(),
            folders_cache: None,
            frame: 0,
        }
    }

    /// Drop every cached texture / listing (the "刷新" button).
    pub fn clear(&mut self) {
        self.textures.clear();
        self.pending.clear();
        self.folders_cache = None;
    }
}

impl Default for PreviewState {
    fn default() -> Self {
        Self::new()
    }
}

/// Preview for the plain image tool: one section, in input order.
pub fn show_files(ui: &mut Ui, files: &[PathBuf], state: &mut PreviewState) {
    let sections = vec![(String::new(), files.to_vec())];
    show_sections(ui, sections, state);
}

/// Preview for the folder batch tool: one section per folder, in input order.
///
/// Listings are re-scanned periodically so newly added images appear without
/// a manual refresh.
pub fn show_folders(ui: &mut Ui, folders: &[PathBuf], state: &mut PreviewState) {
    state.frame += 1;
    let stale = state.frame % RESCAN_FRAMES == 0;
    let cached = state.folders_cache.as_ref().and_then(|(key, value)| {
        if key == folders && !stale {
            Some(value.clone())
        } else {
            None
        }
    });
    let sections = cached.unwrap_or_else(|| {
        let sections: Vec<(String, Vec<PathBuf>)> = folders
            .iter()
            .map(|dir| {
                (
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.display().to_string()),
                    crate::tools::folder_to_pdf::collect_images(dir),
                )
            })
            .collect();
        state.folders_cache = Some((folders.to_vec(), sections.clone()));
        sections
    });
    show_sections(ui, sections, state);
}

/// Shared renderer: header row (expand / collapse, refresh) + thumbnail rows.
fn show_sections(ui: &mut Ui, sections: Vec<(String, Vec<PathBuf>)>, state: &mut PreviewState) {
    theme::card(ui, "页面预览", |ui| {
        let total: usize = sections.iter().map(|(_, paths)| paths.len()).sum();
        ui.horizontal(|ui| {
            let toggle = if state.open { "收起预览" } else { "展开预览" };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            theme::hint(ui, &format!("{total} 张图片"));
        });

        if !state.open || total == 0 {
            if state.open {
                theme::hint(ui, "没有可预览的图片");
            }
            return;
        }

        // Queue anything not yet decoded, then decode within budget.
        for (_, paths) in &sections {
            for path in paths {
                if !state.textures.contains_key(path) && !state.pending.contains(path) {
                    state.pending.push(path.clone());
                }
            }
        }
        let ctx = ui.ctx().clone();
        let mut budget = DECODE_BUDGET;
        let mut still_pending = Vec::new();
        for path in state.pending.drain(..) {
            if state.textures.contains_key(&path) {
                continue;
            }
            if budget > 0 {
                let thumb = decode_thumb(&ctx, &path);
                state.textures.insert(path, thumb);
                budget -= 1;
            } else {
                still_pending.push(path);
            }
        }
        state.pending = still_pending;

        for (title, paths) in &sections {
            if !title.is_empty() {
                ui.add_space(theme::GAP_XS);
                ui.label(RichText::new(title).small().weak());
            }
            egui::ScrollArea::horizontal()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, path) in paths.iter().enumerate() {
                            draw_thumbnail(ui, index + 1, path, state);
                        }
                    });
                });
        }
    });
}

fn draw_thumbnail(ui: &mut Ui, index: usize, path: &Path, state: &PreviewState) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(THUMB_W_MAX, THUMB_H + 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::hover_fill(ui));

    match state.textures.get(path) {
        Some(Thumb::Ready(tex)) => {
            let size = tex.size_vec2();
            let scale = (THUMB_H / size.y)
                .min((THUMB_W_MAX - 6.0) / size.x)
                .min(1.0);
            let draw = size * scale;
            let image_rect = egui::Rect::from_center_size(
                rect.center() - egui::vec2(0.0, 7.0),
                draw,
            );
            painter.image(
                tex.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        Some(Thumb::Failed) => {
            painter.text(
                rect.center() - egui::vec2(0.0, 7.0),
                egui::Align2::CENTER_CENTER,
                "无法预览",
                theme::painted_font(ui),
                theme::warning(ui),
            );
        }
        None => {
            painter.text(
                rect.center() - egui::vec2(0.0, 7.0),
                egui::Align2::CENTER_CENTER,
                "…",
                theme::painted_font(ui),
                ui.visuals().weak_text_color(),
            );
        }
    }

    // 1-based index: this is the number the merge spec refers to.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 7.0),
        egui::Align2::CENTER_CENTER,
        format!("{index}"),
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        ui.visuals().text_color(),
    );

    resp.on_hover_text(path.display().to_string());
}

fn decode_thumb(ctx: &egui::Context, path: &Path) -> Thumb {
    let Ok(img) = image::open(path) else {
        return Thumb::Failed;
    };
    let thumb = img.thumbnail(160, 160);
    let rgba = thumb.to_rgba8();
    let handle = ctx.load_texture(
        format!("thumb-{}", path.display()),
        egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    );
    Thumb::Ready(handle)
}
