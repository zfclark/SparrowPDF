//! Panel: PDF merge.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector, section,
    status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::Merge;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, true);
    ui.label(
        egui::RichText::new("按列表顺序合并，可拖动手柄调整顺序。")
            .small()
            .weak(),
    );

    section(ui, "选项");
    crate::ui::panels::password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "merged.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = form.inputs.len() >= 2;
    action_row(ui, state, can_run, "至少需要 2 个 PDF 文件。")
}
