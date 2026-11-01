//! Shared panel scaffolding.
//!
//! Every tool panel renders the same regions (inputs, options, output) as
//! cards, so the window looks consistent regardless of which tool is active.
//! Panels never touch `lopdf`/`medpdf`/`image` types: they only edit
//! [`FormState`], which is translated into a [`ToolRequest`] by `app`.
//!
//! The sticky chrome (result area, action bar) is owned by the shell, not by
//! panels; each panel reports through [`PanelView`] whether it can run.

pub mod delete_pages;
pub mod encrypt;
pub mod extract;
pub mod folder_to_pdf;
pub mod image_to_pdf;
pub mod image_watermark;
pub mod merge;
pub mod pdf_to_image;
pub mod reorder;
pub mod rotate;
pub mod split;
pub mod text_watermark;

pub use reorder::ReorderState;

use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::{RichText, Ui};

use crate::model::{InputFile, ToolId};
use crate::tools::ToolOptions;
use crate::ui::theme::{self, ROW_H};

/// Mutable UI state for the active tool.
pub struct FormState {
    pub inputs: Vec<InputFile>,
    pub output: Option<PathBuf>,
    pub password: String,
    /// Per-tool option drafts, so switching tools does not lose settings.
    pub options: BTreeMap<ToolId, ToolOptions>,
}

impl FormState {
    pub fn new() -> Self {
        let mut options = BTreeMap::new();
        for id in ToolId::ALL {
            options.insert(id, ToolOptions::default_for(id));
        }
        Self {
            inputs: Vec::new(),
            output: None,
            password: String::new(),
            options,
        }
    }

    /// Reset everything that belongs to a single tool.
    ///
    /// The chosen output path is included on purpose: a folder picked for a
    /// multi-output tool must not leak into a single-output tool (where it is
    /// used verbatim as the target *file*), and a file picked for a
    /// single-output tool must not redirect multi-output tools to its parent.
    pub fn reset_inputs(&mut self) {
        self.inputs.clear();
        self.password.clear();
        self.output = None;
    }

    pub fn options_for(&self, id: ToolId) -> &ToolOptions {
        self.options
            .get(&id)
            .expect("every tool has default options")
    }

    pub fn options_mut(&mut self, id: ToolId) -> &mut ToolOptions {
        self.options
            .get_mut(&id)
            .expect("every tool has default options")
    }
}

impl Default for FormState {
    fn default() -> Self {
        Self::new()
    }
}

/// What the user asked the panel to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAction {
    None,
    Run,
    Cancel,
    /// The result card offered to save into Documents instead of a
    /// protected location.
    RetryInDocuments,
}

/// What a panel tells the shell about its action bar.
#[derive(Debug, Clone, Copy)]
pub struct PanelView {
    /// Whether the execute button should be enabled.
    pub can_run: bool,
    /// Shown next to the button while the tool cannot run.
    pub hint: &'static str,
}

impl PanelView {
    pub fn new(can_run: bool, hint: &'static str) -> Self {
        Self { can_run, hint }
    }
}

/// Readiness of a tool, computed before rendering.
///
/// The shell draws the sticky action bar before the panel exists, so the
/// execute state is decided here rather than inside `show`.
pub fn panel_view(tool: ToolId, form: &FormState, reorder: &ReorderState) -> PanelView {
    let has_input = !form.inputs.is_empty();
    match tool {
        ToolId::ImageToPdf => PanelView::new(has_input, "请先添加至少一张图片。"),
        ToolId::FolderToPdf => PanelView::new(has_input, "请先添加至少一个文件夹。"),
        ToolId::Merge => PanelView::new(form.inputs.len() >= 2, "至少需要 2 个 PDF 文件。"),
        ToolId::Reorder => PanelView::new(
            has_input && !reorder.rows.is_empty(),
            "请选择 PDF，等待页序载入。",
        ),
        ToolId::TextWatermark => {
            let has_text = matches!(
                form.options_for(tool),
                ToolOptions::TextWatermark { text, .. } if !text.trim().is_empty()
            );
            PanelView::new(has_input && has_text, "请选择 PDF 并填写水印文字。")
        }
        ToolId::ImageWatermark => {
            PanelView::new(form.inputs.len() >= 2, "需要 1 个 PDF 和 1 张图片。")
        }
        _ => PanelView::new(has_input, "请先选择一个 PDF 文件。"),
    }
}

/// Pick files via the native dialog and append them, respecting `max_inputs`.
pub fn add_files_button(ui: &mut Ui, tool: ToolId, form: &mut FormState) {
    ui.horizontal(|ui| {
        if theme::button(ui, "添加文件…").clicked() {
            if let Some(paths) = pick_files(tool) {
                append_inputs(tool, form, paths);
            }
        }
        if !form.inputs.is_empty() && theme::small_button(ui, "清空").clicked() {
            form.inputs.clear();
        }
    });
}

/// Pick folders via the native dialog and append them (folder batch tool).
pub fn add_folders_button(ui: &mut Ui, tool: ToolId, form: &mut FormState) {
    ui.horizontal(|ui| {
        if theme::button(ui, "添加文件夹…").clicked() {
            if let Some(paths) = rfd::FileDialog::new().pick_folders() {
                append_inputs(tool, form, paths);
            }
        }
        if !form.inputs.is_empty() && theme::small_button(ui, "清空").clicked() {
            form.inputs.clear();
        }
    });
}

/// Append paths, honouring the tool's input limit and dropping duplicates.
pub fn append_inputs(tool: ToolId, form: &mut FormState, paths: Vec<PathBuf>) {
    for path in paths {
        if let Some(max) = tool.max_inputs() {
            if form.inputs.len() >= max {
                break;
            }
        }
        let file = InputFile::new(path);
        if !form.inputs.iter().any(|f| f.path == file.path) {
            form.inputs.push(file);
        }
    }
}

/// File dialog filtered to the tool's accepted extensions.
fn pick_files(tool: ToolId) -> Option<Vec<PathBuf>> {
    let extensions = tool.accepted_extensions();
    let filter_name = match tool {
        ToolId::ImageToPdf => "图片",
        ToolId::ImageWatermark => "PDF 与图片",
        _ => "PDF",
    };

    let dialog = rfd::FileDialog::new().add_filter(filter_name, extensions);
    let picked = if tool.max_inputs().map(|m| m > 1).unwrap_or(true) {
        dialog.pick_files()
    } else {
        dialog.pick_file().map(|p| vec![p])
    };
    picked
}

/// Save-target picker; returns the chosen path.
fn pick_output(default_name: &str, extensions: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter("输出", extensions)
        .save_file()
}

fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

/// Output selector. `is_dir_tool` switches between a file and a folder picker.
pub fn output_selector(
    ui: &mut Ui,
    form: &mut FormState,
    default_name: &str,
    extensions: &[&str],
    is_dir_tool: bool,
) {
    output_selector_hinted(
        ui,
        form,
        default_name,
        extensions,
        is_dir_tool,
        "未选择，默认保存到源文件所在文件夹（目录不可写时自动改存到「文档」）",
    );
}

/// [`output_selector`] with a custom hint for the not-chosen state.
pub fn output_selector_hinted(
    ui: &mut Ui,
    form: &mut FormState,
    default_name: &str,
    extensions: &[&str],
    is_dir_tool: bool,
    empty_hint: &str,
) {
    ui.horizontal(|ui| {
        let label = if is_dir_tool {
            "选择输出目录…"
        } else {
            "选择输出文件…"
        };
        if theme::button(ui, label).clicked() {
            let picked = if is_dir_tool {
                pick_folder()
            } else {
                pick_output(default_name, extensions)
            };
            if picked.is_some() {
                form.output = picked;
            }
        }
        match &form.output {
            Some(path) => {
                let shown = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                ui.label(RichText::new(shown).weak())
                    .on_hover_text(path.display().to_string());
            }
            None => {
                theme::hint(ui, empty_hint);
            }
        }
    });
}

/// Password field for tools that can open encrypted inputs.
pub fn password_field(ui: &mut Ui, form: &mut FormState) {
    theme::field_row_hover(ui, "打开密码", "文件已加密时填写；未加密可留空", |ui| {
        theme::password_input(ui, &mut form.password, "文件已加密时必填，否则留空");
    });
}

/// File list with per-row remove buttons and optional drag reordering.
pub fn input_list(ui: &mut Ui, tool: ToolId, inputs: &mut Vec<InputFile>, reorderable: bool) {
    input_list_hinted(ui, tool, inputs, reorderable, "拖放文件到此处，或点击「添加文件…」");
}

/// [`input_list`] with a custom empty-state hint (e.g. for folders).
pub fn input_list_hinted(
    ui: &mut Ui,
    tool: ToolId,
    inputs: &mut Vec<InputFile>,
    reorderable: bool,
    empty_hint: &str,
) {
    if inputs.is_empty() {
        theme::drop_zone(ui, empty_hint);
        return;
    }
    input_list_filled(ui, tool, inputs, reorderable);
}

fn input_list_filled(ui: &mut Ui, tool: ToolId, inputs: &mut Vec<InputFile>, reorderable: bool) {
    let mut remove_path: Option<PathBuf> = None;

    if reorderable && inputs.len() > 1 {
        // Drag handle reorders the list in place; removal stays on explicit
        // buttons so a drag never accidentally deletes an entry.
        egui_dnd::dnd(ui, egui::Id::unique(("input-list-dnd", tool))).show_vec(
            inputs,
            |ui, item, handle, _state| {
                ui.horizontal(|ui| {
                    handle.ui(ui, drag_handle);
                    let name = ui.add(
                        egui::Label::new(RichText::new(item.display_name())).truncate(),
                    );
                    name.on_hover_text(item.path.display().to_string());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if remove_button(ui).clicked() {
                            remove_path = Some(item.path.clone());
                        }
                    });
                });
            },
        );
    } else {
        for (index, file) in inputs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(18.0, ROW_H),
                    egui::Label::new(RichText::new(format!("{}.", index + 1)).weak()),
                );
                let name = ui.add(egui::Label::new(RichText::new(file.display_name())).truncate());
                name.on_hover_text(file.path.display().to_string());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if remove_button(ui).clicked() {
                        remove_path = Some(file.path.clone());
                    }
                });
            });
        }
    }

    if let Some(path) = remove_path {
        inputs.retain(|f| f.path != path);
    }
}

fn remove_button(ui: &mut Ui) -> egui::Response {
    theme::small_button(ui, "×").on_hover_text("移除")
}

/// Drag handle drawn with the painter instead of a text glyph.
///
/// The common "⠿" braille character is not covered by egui's default fonts nor
/// by the CJK fallback, so it rendered as a tofu box. Six dots convey the same
/// affordance without any font involvement.
fn drag_handle(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    let center = rect.center();
    let color = ui.visuals().widgets.inactive.fg_stroke.color;
    for row in [-3.5_f32, 0.0, 3.5] {
        for col in [-2.0_f32, 2.0] {
            painter.circle_filled(center + egui::vec2(col, row), 1.1, color);
        }
    }
}

/// Note shown above panels that emit multiple files.
pub fn multi_output_note(ui: &mut Ui) {
    theme::hint(ui, "生成多个文件，输出到所选目录");
}
