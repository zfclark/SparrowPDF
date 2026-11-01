//! Thumbnail page previews for the image→PDF tools.
//!
//! Shows what the produced pages will look like: one thumbnail per image, in
//! final page order, labeled with the **output PDF's page number** — merged
//! images share one label, so numbering stays continuous no matter which
//! images are combined. Decoding is budgeted (a few images per frame) so
//! large folders never stall the UI, and textures are cached per path for
//! the whole session. The area collapses / expands on demand.

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

/// How preview labels are computed for the folder batch tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageNumbering {
    /// Two images share one page.
    pub two_per_page: bool,
    /// Merge only the images listed in `merge_spec`.
    pub use_merge_spec: bool,
    /// The spec text, e.g. `1-2,5-6`.
    pub merge_spec: String,
}

/// One preview group: a (possibly empty) title plus its images, each labeled
/// with its output page number and its own image number.
#[derive(Clone)]
struct Section {
    title: String,
    items: Vec<(PathBuf, usize, usize)>,
}

/// Per-session preview state, owned by the app shell.
pub struct PreviewState {
    /// Expanded / collapsed.
    pub open: bool,
    textures: BTreeMap<PathBuf, Thumb>,
    pending: Vec<PathBuf>,
    /// Cached folder listings for the folder tool, keyed by the folder list
    /// and the numbering settings (either change invalidates the cache).
    folders_cache: Option<(Vec<PathBuf>, String, Vec<Section>)>,
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

/// Preview for the plain image tool: one section, in input order; every
/// image is its own page, so page number == image number.
pub fn show_files(ui: &mut Ui, files: &[PathBuf], state: &mut PreviewState) {
    let items = files
        .iter()
        .enumerate()
        .map(|(i, path)| (path.clone(), i + 1, i + 1))
        .collect();
    show_sections(
        ui,
        vec![Section {
            title: String::new(),
            items,
        }],
        state,
    );
}

/// Preview for the folder batch tool: one section per folder, in input
/// order. Thumbnails are labeled with the output PDF's page number
/// (continuous across merges); the hover tooltip additionally shows each
/// image's own number, which is what the merge spec refers to.
///
/// Listings are re-scanned periodically so newly added images appear without
/// a manual refresh.
pub fn show_folders(
    ui: &mut Ui,
    folders: &[PathBuf],
    numbering: Option<&PageNumbering>,
    state: &mut PreviewState,
) {
    state.frame += 1;
    let key = format!("{numbering:?}");
    let stale = state.frame % RESCAN_FRAMES == 0;
    let cached = state.folders_cache.as_ref().and_then(|(dirs, key_cached, sections)| {
        if dirs == folders && key_cached == &key && !stale {
            Some(sections.to_vec())
        } else {
            None
        }
    });
    let sections = cached.unwrap_or_else(|| {
        let sections: Vec<Section> = folders
            .iter()
            .map(|dir| {
                let images = crate::tools::folder_to_pdf::collect_images(dir);
                let numbers = match numbering {
                    Some(n) => crate::tools::folder_to_pdf::preview_page_numbers(
                        images.len(),
                        n.two_per_page,
                        n.use_merge_spec,
                        &n.merge_spec,
                    ),
                    None => (1..=images.len()).collect(),
                };
                let title = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.display().to_string());
                Section {
                    title,
                    items: images.into_iter().zip(numbers).enumerate().map(|(i, (path, page))| (path, page, i + 1)).collect(),
                }
            })
            .collect();
        state.folders_cache = Some((folders.to_vec(), key, sections.clone()));
        sections
    });
    show_sections(ui, sections, state);
}

/// Shared renderer: header row (expand / collapse, refresh) + thumbnail rows.
fn show_sections(ui: &mut Ui, sections: Vec<Section>, state: &mut PreviewState) {
    theme::card(ui, "页面预览", |ui| {
        let total: usize = sections.iter().map(|s| s.items.len()).sum();
        ui.horizontal(|ui| {
            let toggle = if state.open { "收起预览" } else { "展开预览" };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            theme::hint(ui, &format!("{total} 张图片，标签为输出页码"));
        });

        if !state.open || total == 0 {
            if state.open {
                theme::hint(ui, "没有可预览的图片");
            }
            return;
        }

        // Queue anything not yet decoded, then decode within budget.
        for section in &sections {
            for (path, _, _) in &section.items {
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

        for section in &sections {
            if !section.title.is_empty() {
                ui.add_space(theme::GAP_XS);
                ui.label(RichText::new(&section.title).small().weak());
            }
            egui::ScrollArea::horizontal()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (path, page, image_no) in &section.items {
                            draw_thumbnail(ui, *page, *image_no, path, state);
                        }
                    });
                });
        }
    });
}

fn draw_thumbnail(
    ui: &mut Ui,
    page: usize,
    image_no: usize,
    path: &Path,
    state: &PreviewState,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(THUMB_W_MAX, THUMB_H + 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::hover_fill(ui));

    match state.textures.get(path) {
        Some(Thumb::Ready(tex)) => {
            let size = tex.size_vec2();
            let scale = (THUMB_H / size.y)
                .min((THUMB_W_MAX - 6.0) / size.x)
                .min(1.0);
            let draw = size * scale;
            let image_rect =
                egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 7.0), draw);
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

    // Label = the output PDF's page number; merged images share it.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 7.0),
        egui::Align2::CENTER_CENTER,
        format!("P{page}"),
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        ui.visuals().text_color(),
    );

    resp.on_hover_text(format!("图片 {image_no} · 页 P{page}\n{}", path.display()));
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
