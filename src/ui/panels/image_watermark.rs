//! Panel: image watermark.

use egui::Ui;

use crate::model::{TaskState, ToolId, WatermarkPlacement};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::ImageWatermark;

    section(ui, "输入（第 1 个为 PDF，第 2 个为水印图片）");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::ImageWatermark {
        scale,
        opacity,
        position,
    } = form.options_mut(tool)
    {
        ui.horizontal(|ui| {
            ui.label("缩放");
            ui.add(egui::Slider::new(scale, 0.05..=1.0).fixed_decimals(2));
            ui.label("（相对页面）");
        });
        ui.horizontal(|ui| {
            ui.label("透明度");
            ui.add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2));
        });
        ui.horizontal(|ui| {
            ui.label("位置");
            ui.radio_value(position, WatermarkPlacement::Center, "居中");
            ui.radio_value(position, WatermarkPlacement::Tiled, "平铺");
        });
    }
    password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "image_watermarked.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = form.inputs.len() >= 2;
    action_row(ui, state, can_run, "需要 1 个 PDF 和 1 张图片。")
}
