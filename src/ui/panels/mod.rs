//! Shared panel scaffolding.
//!
//! Every tool panel renders the same four regions (inputs, options, action,
//! status) so the window looks consistent regardless of which tool is active.
//! Panels never touch `lopdf`/`medpdf`/`image` types: they only edit
//! [`FormState`], which is translated into a [`ToolRequest`] by `app`.

pub mod delete_pages;
pub mod encrypt;
pub mod extract;
pub mod extract_text;
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

use egui::Ui;

use crate::model::{InputFile, TaskState, ToolId};
use crate::tools::ToolOptions;

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
    pub fn reset_inputs(&mut self) {
        self.inputs.clear();
        self.password.clear();
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
}

/// Reusable section header.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong());
}

/// Pick files via the native dialog and append them, respecting `max_inputs`.
pub fn add_files_button(ui: &mut Ui, tool: ToolId, form: &mut FormState) {
    ui.horizontal(|ui| {
        if ui.button("添加文件…").clicked() {
            if let Some(paths) = pick_files(tool) {
                append_inputs(tool, form, paths);
            }
        }
        if ui.button("清空").clicked() {
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
    ui.horizontal(|ui| {
        let button = if is_dir_tool { "选择输出目录…" } else { "选择输出文件…" };
        if ui.button(button).clicked() {
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
                ui.label(egui::RichText::new(shown).weak())
                    .on_hover_text(path.display().to_string());
            }
            None => {
                ui.label(egui::RichText::new("（未选择，默认输出到源文件旁）").weak());
            }
        }
    });
}

/// Password field, shown only when the tool can open encrypted inputs.
pub fn password_field(ui: &mut Ui, form: &mut FormState) {
    ui.horizontal(|ui| {
        ui.label("输入密码");
        ui.add(
            egui::TextEdit::singleline(&mut form.password)
                .password(true)
                .desired_width(180.0)
                .hint_text("若文件未加密可留空"),
        );
    });
}

/// File list with per-row remove buttons and optional drag reordering.
pub fn input_list(ui: &mut Ui, tool: ToolId, inputs: &mut Vec<InputFile>, reorderable: bool) {
    if inputs.is_empty() {
        ui.label(egui::RichText::new("尚未添加文件（可拖放或点击“添加文件…”）").weak());
        return;
    }

    let mut remove: Option<usize> = None;

    if reorderable && inputs.len() > 1 {
        // Drag handle reorders the list in place; removal stays on explicit
        // buttons so a drag never accidentally deletes an entry.
        egui_dnd::dnd(ui, egui::Id::unique(("input-list-dnd", tool))).show_vec(
            inputs,
            |ui, item, handle, _state| {
                ui.horizontal(|ui| {
                    handle.ui(ui, |ui| {
                        ui.label("⠿");
                    });
                    ui.label(item.display_name());
                });
            },
        );

        ui.horizontal_wrapped(|ui| {
            for index in 0..inputs.len() {
                if ui.small_button(format!("移除 {}", index + 1)).clicked() {
                    remove = Some(index);
                }
            }
        });
    } else {
        for (index, file) in inputs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}.", index + 1));
                ui.label(file.display_name())
                    .on_hover_text(file.path.display().to_string());
                if ui.small_button("移除").clicked() {
                    remove = Some(index);
                }
            });
        }
    }

    if let Some(index) = remove {
        if index < inputs.len() {
            inputs.remove(index);
        }
    }
}

/// Execute / cancel buttons plus a preview of what will happen.
pub fn action_row(
    ui: &mut Ui,
    state: &TaskState,
    can_run: bool,
    hint: &str,
) -> PanelAction {
    let mut action = PanelAction::None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let busy = state.is_busy();
        ui.add_enabled_ui(!busy && can_run, |ui| {
            if ui.button("执行").clicked() {
                action = PanelAction::Run;
            }
        });
        if busy {
            if ui.button("取消").clicked() {
                action = PanelAction::Cancel;
            }
            ui.spinner();
            ui.label(egui::RichText::new(state.label()).weak());
        }
    });
    if !can_run && !state.is_busy() {
        ui.label(egui::RichText::new(hint).weak());
    }
    action
}

/// Per-panel result area, mirroring `TaskState` for the active tool.
pub fn status_area(ui: &mut Ui, state: &TaskState) {
    match state {
        TaskState::Running { progress, message } => {
            ui.add(egui::ProgressBar::new(*progress).show_percentage());
            ui.label(message);
        }
        TaskState::Done { summary, outputs } => {
            ui.label(egui::RichText::new(summary).strong());
            for output in outputs.iter().take(8) {
                if ui
                    .link(output.display().to_string())
                    .clicked()
                {
                    // Best effort: hand the path to the OS.
                    let _ = opener_open(output);
                }
            }
        }
        TaskState::Failed { error } => {
            ui.colored_label(egui::Color32::LIGHT_RED, error.to_string());
        }
        TaskState::Cancelled => {
            ui.label(egui::RichText::new("已取消").weak());
        }
        TaskState::Idle | TaskState::Validating | TaskState::Ready => {}
    }
}

/// Open a path with the platform default handler.
fn opener_open(path: &std::path::Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// Note shown above panels that emit multiple files.
pub fn multi_output_note(ui: &mut Ui) {
    ui.label(
        egui::RichText::new("本工具会生成多个文件，将输出到所选目录。")
            .small()
            .weak(),
    );
}
