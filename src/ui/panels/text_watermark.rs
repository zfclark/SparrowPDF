//! Panel: text watermark.

use egui::Ui;

use crate::model::{TaskState, ToolId, WatermarkPlacement};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector,
    password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::TextWatermark;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::TextWatermark {
        text,
        font_size,
        color,
        opacity,
        rotation,
        placement,
    } = form.options_mut(tool)
    {
        ui.horizontal(|ui| {
            ui.label("水印文字");
            ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(200.0)
                    .hint_text("如 内部资料"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("字号");
            ui.add(egui::DragValue::new(font_size).range(6.0..=200.0).speed(1.0));
            ui.label("颜色");
            ui.color_edit_button_srgb(color);
        });
        ui.horizontal(|ui| {
            ui.label("透明度");
            ui.add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2));
        });
        ui.horizontal(|ui| {
            ui.label("旋转角度");
            ui.add(egui::DragValue::new(rotation).range(-180.0..=180.0).speed(1.0));
            ui.label("°");
        });
        ui.horizontal(|ui| {
            ui.label("排布");
            ui.radio_value(placement, WatermarkPlacement::Center, "居中");
            ui.radio_value(placement, WatermarkPlacement::Tiled, "平铺");
        });
    }
    ui.label(
        egui::RichText::new("中文水印需要系统安装中文字体（如微软雅黑、思源黑体）。")
            .small()
            .weak(),
    );
    password_field(ui, form);

    section(ui, "输出");
    output_selector(ui, form, "watermarked.pdf", &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let has_text = matches!(
        form.options_for(tool),
        ToolOptions::TextWatermark { text, .. } if !text.trim().is_empty()
    );
    let can_run = !form.inputs.is_empty() && has_text;
    action_row(ui, state, can_run, "请选择 PDF 并填写水印文字。")
}
