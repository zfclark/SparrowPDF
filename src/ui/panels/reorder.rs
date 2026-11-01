//! Panel: reorder pages.
//!
//! The page order is kept as a drag list of page labels. `egui_dnd` reorders the
//! list; the tool receives the resulting 0-based source indices. The list loads
//! itself as soon as a file is staged (see [`ensure_loaded`]), so reordering is
//! drag-only.

use std::path::PathBuf;

use egui::Ui;

use crate::model::ToolId;
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, drag_handle, input_list, output_selector, password_field,
};
use crate::ui::theme;

/// A row in the page-order list.
#[derive(Debug, Clone, Hash)]
pub struct PageRow {
    /// 0-based source page index.
    pub source: usize,
    /// 1-based label shown to the user.
    pub label: String,
}

/// Per-panel drag state, kept outside `FormState` because it is transient.
#[derive(Default)]
pub struct ReorderState {
    pub rows: Vec<PageRow>,
    /// Which input file the rows were built from.
    pub built_for: Option<PathBuf>,
    pub total_pages: u32,
    /// Last `(file, password)` pair the page count was read with, so a new
    /// password retriggers a load without looping on failure.
    pub last_attempt: Option<(PathBuf, String)>,
}

pub fn show(ui: &mut Ui, form: &mut FormState, reorder: &mut ReorderState) {
    let tool = ToolId::Reorder;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "页面顺序", |ui| {
        if reorder.rows.is_empty() {
            if reorder.last_attempt.is_some() {
                theme::hint(ui, "无法读取页数：请检查文件是否损坏，或先填写打开密码");
            } else {
                theme::hint(ui, "选择 PDF 后自动载入页面，拖动即可调整顺序");
            }
        } else {
            ui.horizontal(|ui| {
                if theme::small_button(ui, "重新载入").clicked() {
                    reorder.last_attempt = None;
                    load_rows(form, reorder);
                }
                if theme::small_button(ui, "恢复原序").clicked() {
                    reorder.rows = default_rows(reorder.total_pages);
                }
                theme::hint(ui, &format!("共 {} 页", reorder.total_pages));
            });
            egui_dnd::dnd(ui, egui::Id::unique("reorder-pages")).show_vec(
                &mut reorder.rows,
                |ui, item, handle, _state| {
                    ui.horizontal(|ui| {
                        handle.ui(ui, drag_handle);
                        ui.label(item.label.clone());
                    });
                },
            );
            // Push the visible order into the tool options.
            if let ToolOptions::Reorder { order } = form.options_mut(tool) {
                *order = reorder.rows.iter().map(|r| r.source).collect();
            }
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "reordered.pdf", &["pdf"], false);
    });
}

/// Load the page list on first sight of a file (or a new password).
///
/// Called by the shell before the action bar is drawn, so the execute button
/// never lags a frame behind the drag list.
pub fn ensure_loaded(form: &mut FormState, reorder: &mut ReorderState) {
    let Some(input) = form.inputs.first() else {
        *reorder = ReorderState::default();
        return;
    };
    let attempt = (input.path.clone(), form.password.clone());
    if reorder.rows.is_empty() && reorder.last_attempt.as_ref() != Some(&attempt) {
        load_rows(form, reorder);
    }
}

fn default_rows(total_pages: u32) -> Vec<PageRow> {
    (0..total_pages as usize)
        .map(|i| PageRow {
            source: i,
            label: format!("第 {} 页", i + 1),
        })
        .collect()
}

/// Rebuild the page list from the selected file's page count.
fn load_rows(form: &mut FormState, reorder: &mut ReorderState) {
    let Some(input) = form.inputs.first() else {
        return;
    };
    let path = input.path.clone();
    let password = if form.password.is_empty() {
        None
    } else {
        Some(form.password.as_str())
    };

    reorder.last_attempt = Some((path.clone(), form.password.clone()));

    // Page count is read through the engine, so no `lopdf` type reaches the UI.
    let total = crate::engine::pdf_io::page_count_of(&path, password).unwrap_or(0);
    reorder.total_pages = total;
    if total == 0 {
        reorder.rows.clear();
        return;
    }
    reorder.built_for = Some(path);
    reorder.rows = default_rows(total);
}
