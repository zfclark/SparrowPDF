//! Thumbnail page previews for the image→PDF tools and the page-operation
//! tools.
//!
//! * Image inputs: one thumbnail per image, labeled with the output PDF's
//!   page number (merged images share one label).
//! * PDF inputs (page operations): one thumbnail per page, rasterized by the
//!   built-in renderer with a per-frame budget so large documents never
//!   stall the UI.
//! * Reorder: the same PDF pages, laid out in the order the user is
//!   composing, labeled with the output page number.
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
/// How many PDF pages may be rasterized per batch. The rasterizer re-opens the
/// document for every call, so a bigger batch trades a slightly longer frame
/// for far fewer full parses on long documents (2 pages per call meant one
/// re-parse per frame for the whole strip).
const PDF_BUDGET: usize = 8;
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

/// One thumbnail: what to render, and how it is labeled.
#[derive(Clone)]
struct Item {
    key: PageKey,
    /// Output page number shown under the thumbnail.
    page: usize,
    /// 1-based image number inside the source folder. The merge spec refers to
    /// this number, and it differs from the page label whenever two images
    /// share a page, so both must be shown.
    image: Option<usize>,
}

impl Item {
    fn page(key: PageKey, page: usize) -> Self {
        Self {
            key,
            page,
            image: None,
        }
    }
}

/// One preview group: a (possibly empty) title, an optional error note and
/// its pages, each labeled with its output page number.
#[derive(Clone)]
struct Section {
    title: String,
    note: Option<String>,
    items: Vec<Item>,
}

/// Per-session preview state, owned by the app shell.
pub struct PreviewState {
    /// Expanded / collapsed.
    pub open: bool,
    textures: BTreeMap<PageKey, Thumb>,
    pending: Vec<PageKey>,
    /// Page count (or error) per PDF, so files are opened once.
    pdf_meta: BTreeMap<PathBuf, Result<u32, String>>,
    /// Password each PDF was opened with, so a new password invalidates the
    /// cached open result instead of showing the previous failure forever.
    pdf_password: BTreeMap<PathBuf, Option<String>>,
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
            pdf_password: BTreeMap::new(),
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
        self.pdf_password.clear();
        self.pdf_pending.clear();
        self.folders_cache = None;
    }

    /// Forget everything cached for one PDF, so it is opened (and rendered)
    /// again — used when the supplied password changes.
    fn forget_pdf(&mut self, path: &Path) {
        self.pdf_meta.remove(path);
        self.pdf_password.remove(path);
        self.pdf_pending.retain(|(queued, _)| queued != path);
        self.pending
            .retain(|key| !matches!(key, PageKey::PdfPage(queued, _) if queued == path));
        self.textures
            .retain(|key, _| !matches!(key, PageKey::PdfPage(queued, _) if queued == path));
    }

    /// Page count of `path`, opening the document at most once per password.
    fn page_total(&mut self, path: &Path, password: Option<&str>) -> Result<u32, String> {
        let password = password.map(str::to_string);
        if self.pdf_password.get(path) != Some(&password) {
            self.forget_pdf(path);
            self.pdf_password
                .insert(path.to_path_buf(), password.clone());
        }
        if let Some(meta) = self.pdf_meta.get(path) {
            return meta.clone();
        }
        let meta = match crate::engine::pdf_io::open_document(path, password.as_deref()) {
            Ok(doc) => Ok(crate::engine::pdf_io::page_count(&doc)),
            Err(err) => Err(err.to_string()),
        };
        self.pdf_meta.insert(path.to_path_buf(), meta.clone());
        meta
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
    if files.is_empty() {
        // An empty preview card was pure noise on a fresh panel: the input card
        // already shows the drop zone and its own empty state.
        return;
    }
    let items = files
        .iter()
        .enumerate()
        .map(|(i, path)| Item {
            key: PageKey::Image(path.clone()),
            page: i + 1,
            image: Some(i + 1),
        })
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
    if folders.is_empty() {
        return;
    }
    state.frame += 1;
    let key = format!("{numbering:?}");
    let stale = state.frame.is_multiple_of(RESCAN_FRAMES);
    let cached = state
        .folders_cache
        .as_ref()
        .and_then(|(dirs, key_cached, sections)| {
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
                let note = if images.is_empty() {
                    // A folder that contributes nothing is the most common
                    // "why is this page empty" question; say it here instead of
                    // showing a bare caption.
                    Some("该文件夹没有可用的 JPG / PNG 图片".to_string())
                } else {
                    None
                };
                Section {
                    title,
                    note,
                    items: images
                        .into_iter()
                        .enumerate()
                        .zip(numbers)
                        .map(|((index, path), page)| Item {
                            key: PageKey::Image(path),
                            page,
                            image: Some(index + 1),
                        })
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
pub fn show_pdf_pages(
    ui: &mut Ui,
    files: &[PathBuf],
    password: Option<&str>,
    state: &mut PreviewState,
) {
    if files.is_empty() {
        return;
    }
    state.frame += 1;
    theme::card(ui, "页面预览", |ui| {
        let total_pages: u32 = files
            .iter()
            .filter_map(|f| state.pdf_meta.get(f))
            .filter_map(|meta| meta.as_ref().ok().copied())
            .sum();
        ui.horizontal(|ui| {
            let toggle = if state.open {
                "收起预览"
            } else {
                "展开预览"
            };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            if state.open && !state.pdf_pending.is_empty() {
                theme::hint(ui, "正在生成页面预览…");
            } else {
                theme::hint(
                    ui,
                    &format!("{total_pages} 页 · 内置渲染器，未嵌入字体的文字可能不显示"),
                );
            }
        });

        if !state.open {
            return;
        }

        // Open at most one new file per frame (large PDFs open slowly).
        //
        // The password matters here: an encrypted input used to be reported as
        // "无法预览：需要密码" even after the user typed the right one, because
        // the preview opened every file without credentials.
        if let Some(file) = files.iter().find(|f| !state.pdf_meta.contains_key(*f)) {
            let opened = state.page_total(file, password);
            state.pdf_meta.insert(file.clone(), opened);
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
                            !state
                                .textures
                                .contains_key(&PageKey::PdfPage(file.clone(), *p))
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
                            .map(|p| Item::page(PageKey::PdfPage(file.clone(), p), p as usize))
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

/// Live preview for the reorder tool: the source pages laid out in the order
/// currently being composed, labeled with the output page number.
///
/// The page sequence comes from the same helper the tool uses to copy pages
/// ([`crate::tools::reorder::ordered_pages`]), and each thumbnail is the real
/// rasterized page (including its `/Rotate`), so what is shown is what the
/// saved PDF will contain. Source pages are cached per page number, so
/// dragging a row re-orders the strip without re-rasterizing anything.
pub fn show_page_order(
    ui: &mut Ui,
    path: Option<&Path>,
    order: &[usize],
    password: Option<&str>,
    state: &mut PreviewState,
) {
    state.frame += 1;
    theme::card(ui, "重排预览", |ui| {
        ui.horizontal(|ui| {
            let toggle = if state.open {
                "收起预览"
            } else {
                "展开预览"
            };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            if state.open && !state.pdf_pending.is_empty() {
                theme::hint(ui, "正在生成页面预览…");
            } else {
                theme::hint(ui, &format!("{} 页，标签为输出页码", order.len()));
            }
        });

        if !state.open {
            return;
        }
        let Some(path) = path else {
            theme::hint(ui, "选择 PDF 后显示重排预览");
            return;
        };
        if order.is_empty() {
            theme::hint(ui, "页序载入后显示预览");
            return;
        }

        let total = match state.page_total(path, password) {
            Ok(0) => {
                theme::hint(ui, "该文件没有页面");
                return;
            }
            Ok(total) => total,
            Err(err) => {
                theme::hint(ui, &format!("无法预览：{err}"));
                return;
            }
        };

        // Output pages in the composed order; out-of-range indices are
        // dropped here and rejected with a message by the run itself.
        let pages = crate::tools::reorder::ordered_pages(order, total);
        if pages.is_empty() {
            theme::hint(ui, "页序里没有可预览的页面");
            return;
        }

        queue_missing_pages(state, path, &pages);
        let section = Section {
            title: String::new(),
            note: None,
            items: pages
                .iter()
                .enumerate()
                .map(|(index, page)| {
                    // Position in the output document, 1-based.
                    Item::page(PageKey::PdfPage(path.to_path_buf(), *page), index + 1)
                })
                .collect(),
        };
        draw_section(ui, &section, state);
        render_pending_pdf_pages(ui.ctx().clone(), state);
    });
}

/// Queue PDF pages that have neither a texture nor a pending slot yet.
fn queue_missing_pages(state: &mut PreviewState, path: &Path, pages: &[u32]) {
    for page in pages {
        let key = PageKey::PdfPage(path.to_path_buf(), *page);
        if state.textures.contains_key(&key) {
            continue;
        }
        if state
            .pdf_pending
            .iter()
            .any(|(queued, list)| queued == path && list.contains(page))
        {
            continue;
        }
        match state
            .pdf_pending
            .iter_mut()
            .find(|(queued, _)| queued == path)
        {
            Some((_, list)) => list.push(*page),
            None => state.pdf_pending.push((path.to_path_buf(), vec![*page])),
        }
    }
}

/// Shared renderer: header row (expand / collapse, refresh) + thumbnail rows.
fn show_sections(ui: &mut Ui, sections: Vec<Section>, state: &mut PreviewState) {
    theme::card(ui, "页面预览", |ui| {
        let total: usize = sections.iter().map(|s| s.items.len()).sum();
        ui.horizontal(|ui| {
            let toggle = if state.open {
                "收起预览"
            } else {
                "展开预览"
            };
            if theme::small_button(ui, toggle).clicked() {
                state.open = !state.open;
            }
            if state.open && theme::small_button(ui, "刷新").clicked() {
                state.clear();
            }
            theme::hint(ui, &format!("{total} 张图片，标签为输出页码"));
        });

        if !state.open {
            return;
        }
        // Nothing to draw and nothing to explain: keep the card short. A
        // section that carries a note (an empty folder, an unreadable PDF) is
        // still rendered below so the reason is visible.
        if total == 0 && sections.iter().all(|section| section.note.is_none()) {
            theme::hint(ui, "没有可预览的图片");
            return;
        }

        // Queue anything not yet decoded, then decode within budget.
        for section in &sections {
            for item in &section.items {
                if !state.textures.contains_key(&item.key) && !state.pending.contains(&item.key) {
                    state.pending.push(item.key.clone());
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
                for item in &section.items {
                    draw_thumbnail(ui, item, state);
                }
            });
        });
}

fn draw_thumbnail(ui: &mut Ui, item: &Item, state: &PreviewState) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(THUMB_W_MAX, THUMB_H + 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::hover_fill(ui));

    match state.textures.get(&item.key) {
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
        format!("P{}", item.page),
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        ui.visuals().text_color(),
    );

    // Image number and page number are two different things as soon as two
    // images share a page; the merge spec refers to the *image* number, so the
    // tooltip spells both out.
    let hover = match &item.key {
        PageKey::Image(path) => match item.image {
            Some(image) if image != item.page => {
                format!("图片 {image} · 页 P{}\n{}", item.page, path.display())
            }
            Some(image) => format!("图片 {image} · 页 P{}\n{}", item.page, path.display()),
            None => format!("页 P{}\n{}", item.page, path.display()),
        },
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
    // Encrypted inputs only rasterize with the password the UI collected.
    let password = state.pdf_password.get(&path).cloned().flatten();

    match crate::engine::render::render_page_thumbnails(
        &path,
        password.as_deref(),
        &batch,
        THUMB_H,
        fonts,
    ) {
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
    if !state.pdf_pending.is_empty() {
        // Keep frames coming so the strip fills in without user input.
        ctx.request_repaint();
    }
}
