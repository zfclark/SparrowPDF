//! Thumbnail page previews for the image→PDF tools and the page-operation
//! tools.
//!
//! * Image inputs: one thumbnail per image, labeled with the output PDF's
//!   page number (merged images share one label).
//! * PDF inputs (page operations): one thumbnail per page, rasterized by the
//!   built-in renderer with a per-frame budget so large documents never
//!   stall the UI.
//!
//! Textures are cached per page key for the whole session; the area
//! collapses / expands on demand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Sense, Ui, Vec2};

use crate::ui::theme;

/// Thumbnail box height; the width adapts to the page aspect ratio.
const THUMB_H: f32 = 110.0;
/// Thumbnail box width cap (portrait pages never get wider than this).
const THUMB_W_MAX: f32 = 96.0;
/// How many new image thumbnails may be decoded per frame.
const IMAGE_BUDGET: usize = 3;
/// How many PDF pages may be rasterized per frame (rasterizing is heavy).
const PDF_BUDGET: usize = 2;
/// Re-scan folder contents / re-open PDFs every N frames while the preview
/// is open, so newly added inputs show up without a manual refresh.
const RESCAN_FRAMES: u64 = 120;

enum Thumb {
    Ready(egui::TextureHandle),
    Failed,
}

/// Identifies one previewable page.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PageKey {
    /// An image file (image→PDF tools).
    Image(PathBuf),
    /// One page (1-based) of a PDF file (page-operation tools).
    PdfPage(PathBuf, u32),
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

/// One preview group: a (possibly empty) title, an optional error note and
/// its pages, each labeled with its output page number.
#[derive(Clone)]
struct Section {
    title: String,
    note: Option<String>,
    items: Vec<(PageKey, usize)>,
}

/// Per-session preview state, owned by the app shell.
pub struct PreviewState {
    /// Expanded / collapsed.
    pub open: bool,
    textures: BTreeMap<PageKey, Thumb>,
    pending: Vec<PageKey>,
    /// Page count (or error) per PDF, so files are opened once.
    pdf_meta: BTreeMap<PathBuf, Result<u32, String>>,
    /// PDF pages waiting to be rasterized, grouped per file.
    pdf_pending: Vec<(PathBuf, Vec<u32>)>,
    /// Cached rasterizer fonts (opened from disk once per session).
    fonts: Option<crate::engine::render::FontSet>,
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
            pdf_meta: BTreeMap::new(),
            pdf_pending: Vec::new(),
            fonts: None,
            folders_cache: None,
            frame: 0,
        }
    }

    /// Drop every cached texture / listing (the "刷新" button).
    pub fn clear(&mut self) {
        self.textures.clear();
        self.pending.clear();
        self.pdf_meta.clear();
        self.pdf_pending.clear();
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
        .map(|(i, path)| (PageKey::Image(path.clone()), i + 1))
        .collect();
    show_sections(
        ui,
        vec![Section {
            title: String::new(),
            note: None,
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
                    note: None,
                    items: images
                        .into_iter()
                        .zip(numbers)
                        .map(|(path, page)| (PageKey::Image(path), page))
                        .collect(),
                }
            })
            .collect();
        state.folders_cache = Some((folders.to_vec(), key, sections.clone()));
        sections
    });
    show_sections(ui, sections, state);
}

/// Preview for the page-operation tools: one section per PDF, thumbnails for
/// every page. Files are opened once and pages are rasterized with a
/// per-frame budget; encrypted or broken files show a note instead.
pub fn show_pdf_pages(ui: &mut Ui, files: &[PathBuf], state: &mut PreviewState) {
    state.frame += 1;
    theme::card(ui, "页面预览", |ui| {
        let total_pages: u32 = files
            .iter()
            .filter_map(|f| state.pdf_meta.get(f))
            .filter_map(|meta| meta.as_ref().ok().copied())
            .sum();
        ui.horizontal(|ui| {
            let toggle = if state.open { "收起预览" } else { "展开预览" };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            if state.open && !state.pdf_pending.is_empty() {
                theme::hint(ui, "正在生成页面预览…");
            } else {
                theme::hint(ui, &format!("{total_pages} 页"));
            }
        });

        if !state.open {
            return;
        }
        if files.is_empty() {
            theme::hint(ui, "没有可预览的文件");
            return;
        }

        // Open at most one new file per frame (large PDFs open slowly).
        if let Some(file) = files.iter().find(|f| !state.pdf_meta.contains_key(*f)) {
            state.pdf_meta.insert(
                file.clone(),
                match crate::engine::pdf_io::open_document(file, None) {
                    Ok(doc) => Ok(crate::engine::pdf_io::page_count(&doc)),
                    Err(err) => Err(err.to_string()),
                },
            );
        }

        let mut sections: Vec<Section> = Vec::new();
        for file in files {
            let Some(meta) = state.pdf_meta.get(file) else {
                continue;
            };
            let title = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.display().to_string());
            match meta {
                Ok(pages) if *pages > 0 => {
                    // Queue pages that have no texture yet (in page order).
                    let mut missing: Vec<u32> = (1..=*pages)
                        .filter(|p| {
                            !state.textures.contains_key(&PageKey::PdfPage(file.clone(), *p))
                                && !state
                                    .pdf_pending
                                    .iter()
                                    .any(|(f, list)| f == file && list.contains(p))
                        })
                        .collect();
                    if !missing.is_empty() {
                        match state.pdf_pending.iter_mut().find(|(f, _)| f == file) {
                            Some((_, list)) => list.append(&mut missing),
                            None => state.pdf_pending.push((file.clone(), missing)),
                        }
                    }
                    sections.push(Section {
                        title,
                        note: None,
                        items: (1..=*pages)
                            .map(|p| (PageKey::PdfPage(file.clone(), p), p as usize))
                            .collect(),
                    });
                }
                Ok(_) => sections.push(Section {
                    title,
                    note: Some("该文件没有页面".to_string()),
                    items: Vec::new(),
                }),
                Err(err) => sections.push(Section {
                    title,
                    note: Some(format!("无法预览：{err}")),
                    items: Vec::new(),
                }),
            }
        }

        for section in &sections {
            draw_section(ui, section, state);
        }

        // Rasterize a small batch per frame, one file at a time.
        render_pending_pdf_pages(ui.ctx().clone(), state);
    });
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
            for (key, _) in &section.items {
                if !state.textures.contains_key(key) && !state.pending.contains(key) {
                    state.pending.push(key.clone());
                }
            }
        }
        let ctx = ui.ctx().clone();
        let mut budget = IMAGE_BUDGET;
        let mut still_pending = Vec::new();
        for key in state.pending.drain(..) {
            if state.textures.contains_key(&key) {
                continue;
            }
            if budget > 0 {
                let thumb = match &key {
                    PageKey::Image(path) => decode_thumb(&ctx, path),
                    PageKey::PdfPage(..) => Thumb::Failed,
                };
                state.textures.insert(key, thumb);
                budget -= 1;
            } else {
                still_pending.push(key);
            }
        }
        state.pending = still_pending;

        for section in &sections {
            draw_section(ui, section, state);
        }
    });
}

fn draw_section(ui: &mut Ui, section: &Section, state: &PreviewState) {
    if !section.title.is_empty() {
        ui.add_space(theme::GAP_XS);
        ui.label(RichText::new(&section.title).small().weak());
    }
    if let Some(note) = &section.note {
        theme::hint(ui, note);
    }
    if section.items.is_empty() {
        return;
    }
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (key, label) in &section.items {
                    draw_thumbnail(ui, key, *label, state);
                }
            });
        });
}

fn draw_thumbnail(ui: &mut Ui, key: &PageKey, label: usize, state: &PreviewState) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(THUMB_W_MAX, THUMB_H + 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::hover_fill(ui));

    match state.textures.get(key) {
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
        format!("P{label}"),
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        ui.visuals().text_color(),
    );

    let hover = match key {
        PageKey::Image(path) => format!("图片 {label} · 页 P{label}\n{}", path.display()),
        PageKey::PdfPage(path, page) => {
            format!("第 {page} 页\n{}", path.display())
        }
    };
    resp.on_hover_text(hover);
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

/// Rasterize the next batch of queued PDF pages (one file per frame).
fn render_pending_pdf_pages(ctx: egui::Context, state: &mut PreviewState) {
    if state.pdf_pending.is_empty() {
        return;
    }
    if state.fonts.is_none() {
        state.fonts = Some(crate::engine::render::FontSet::system());
    }
    let Some(fonts) = state.fonts.as_ref() else {
        return;
    };

    // Process one file per frame: take its queued pages, render up to
    // PDF_BUDGET of them, re-queue the rest.
    let index = state.pdf_pending.len() - 1;
    let (path, pages) = state.pdf_pending.swap_remove(index);
    let batch: Vec<u32> = pages.iter().take(PDF_BUDGET).copied().collect();
    let rest: Vec<u32> = pages.iter().skip(PDF_BUDGET).copied().collect();

    match crate::engine::render::render_page_thumbnails(&path, None, &batch, THUMB_H, fonts) {
        Ok(rendered) => {
            for (page, pixmap) in rendered {
                let handle = ctx.load_texture(
                    format!("page-{}-{page}", path.display()),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [pixmap.width as usize, pixmap.height as usize],
                        &pixmap.data,
                    ),
                    egui::TextureOptions::LINEAR,
                );
                state
                    .textures
                    .insert(PageKey::PdfPage(path.clone(), page), Thumb::Ready(handle));
            }
        }
        Err(err) => {
            crate::engine::logging::log("WARN", &format!("页面预览渲染失败：{err}"));
            // Mark the batch as failed so it does not retry forever.
            for page in batch {
                state
                    .textures
                    .insert(PageKey::PdfPage(path.clone(), page), Thumb::Failed);
            }
        }
    }
    if !rest.is_empty() {
        state.pdf_pending.push((path, rest));
    }
}
