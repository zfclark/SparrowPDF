//! Panel: reorder pages.
//!
//! The page order is kept as a drag list of page labels. `egui_dnd` reorders the
//! list; the tool receives the resulting 0-based source indices.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

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
    pub built_for: Option<std::path::PathBuf>,
    pub total_pages: u32,
}

pub fn show(
    ui: &mut Ui,
    form: &mut FormState,
    state: &TaskState,
    reorder: &mut ReorderState,
) -> PanelAction {
    let tool = ToolId::Reorder;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "页面顺序");
    ui.horizontal(|ui| {
        if ui.button("载入页序").clicked() {
            load_rows(form, reorder);
        }
        if !reorder.rows.is_empty() && ui.button("恢复原序").clicked() {
            reorder.rows = (0..reorder.total_pages as usize)
                .map(|i| PageRow {
                    source: i,
                    label: format!("第 {} 页", i + 1),
                })
                .collect();
        }
    });

    if reorder.rows.is_empty() {
        ui.label(egui::RichText::new("点击“载入页序”后拖动调整。").weak());
    } else {
        egui_dnd::dnd(ui, egui::Id::unique("reorder-pages")).show_vec(
            &mut reorder.rows,
            |ui, item, handle, _state| {
                ui.horizontal(|ui| {
                    handle.ui(ui, |ui| {
                        ui.label("⠿");
                    });
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

    section(ui, "输出");
    output_selector(ui, form, "reordered.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty() && !reorder.rows.is_empty();
    action_row(ui, state, can_run, "请选择 PDF 并载入页序。")
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

    // Page count is read through the engine, so no `lopdf` type reaches the UI.
    let total = crate::engine::pdf_io::page_count_of(&path, password).unwrap_or(0);

    reorder.total_pages = total;
    reorder.built_for = Some(path);
    reorder.rows = (0..total as usize)
        .map(|i| PageRow {
            source: i,
            label: format!("第 {} 页", i + 1),
        })
        .collect();
}
