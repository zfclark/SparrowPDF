//! Panel: rotate pages.

use egui::Ui;

use crate::model::{RotationDegrees, TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::Rotate;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::Rotate { pages, degrees } = form.options_mut(tool) {
        ui.horizontal(|ui| {
            ui.label("页码范围");
            ui.add(
                egui::TextEdit::singleline(pages)
                    .desired_width(140.0)
                    .hint_text("all 或 1-3,7"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("旋转角度");
            ui.radio_value(degrees, RotationDegrees::Deg90, "90°");
            ui.radio_value(degrees, RotationDegrees::Deg180, "180°");
            ui.radio_value(degrees, RotationDegrees::Deg270, "270°");
        });
        ui.label(
            egui::RichText::new("角度为增量：已有 90° 的页面再转 90° 会变成 180°。")
                .small()
                .weak(),
        );
    }
    password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "rotated.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
