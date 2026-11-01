//! Panel: extract pages.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::Extract;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::Extract { pages } = form.options_mut(tool) {
        ui.horizontal(|ui| {
            ui.label("提取页码");
            ui.add(
                egui::TextEdit::singleline(pages)
                    .desired_width(160.0)
                    .hint_text("如 1-3,7"),
            );
        });
    }
    password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "extracted.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
