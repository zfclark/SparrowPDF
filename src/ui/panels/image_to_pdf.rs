//! Panel: images -> PDF.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::tools::Orientation;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector, section,
    status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::ImageToPdf;

    section(ui, "输入图片");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, true);
    ui.label(
        egui::RichText::new("支持 JPEG / PNG，可拖放排序；重复图片会自动去重。")
            .small()
            .weak(),
    );

    section(ui, "选项");
    let mut force = None;
    if let ToolOptions::ImageToPdf { force_orientation } = form.options_mut(tool) {
        ui.horizontal(|ui| {
            ui.label("页面方向");
            ui.radio_value(&mut force, None, "按图片");
            ui.radio_value(&mut force, Some(Orientation::Portrait), "A4 竖版");
            ui.radio_value(&mut force, Some(Orientation::Landscape), "A4 横版");
        });
        *force_orientation = force;
    }

    section(ui, "输出");
    output_selector(ui, form, "merged.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先添加至少一张图片。")
}
