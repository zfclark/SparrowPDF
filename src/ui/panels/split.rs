//! Panel: split.

use egui::Ui;

use crate::model::{PageRangeMode, TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, multi_output_note,
    output_selector, password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::Split;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::Split {
        mode,
        spec,
        every_n,
    } = form.options_mut(tool)
    {
        ui.horizontal(|ui| {
            ui.radio_value(mode, PageRangeMode::Explicit, "按页码范围");
            ui.radio_value(mode, PageRangeMode::EveryN, "按每 N 页");
            ui.radio_value(mode, PageRangeMode::SinglePages, "按单页");
        });
        match mode {
            PageRangeMode::Explicit => {
                ui.horizontal(|ui| {
                    ui.label("页码范围");
                    ui.add(
                        egui::TextEdit::singleline(spec)
                            .desired_width(160.0)
                            .hint_text("如 1-3,7"),
                    );
                });
            }
            PageRangeMode::EveryN => {
                ui.horizontal(|ui| {
                    ui.label("每");
                    ui.add(egui::DragValue::new(every_n).range(1..=9999));
                    ui.label("页一个文件");
                });
            }
            PageRangeMode::SinglePages => {
                ui.label(egui::RichText::new("每一页输出一个 PDF 文件").weak());
            }
        }
    }
    password_field(ui, form);

    section(ui, "输出");
    multi_output_note(ui);
    output_selector(ui, form, "", &[], true);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
