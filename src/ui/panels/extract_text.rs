//! Panel: text extraction.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::ExtractText;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    ui.label(
        egui::RichText::new("输出纯文本（UTF-8）。仅对含文本层的 PDF 有效，扫描件无结果。")
            .small()
            .weak(),
    );
    password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "extracted.txt", &["txt"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
