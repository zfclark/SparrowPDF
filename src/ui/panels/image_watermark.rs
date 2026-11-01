//! Panel: image watermark.

use egui::Ui;

use crate::model::{ToolId, WatermarkPlacement};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::ImageWatermark;

    theme::card(ui, "输入（第 1 个为 PDF，第 2 个为水印图片）", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::ImageWatermark {
            scale,
            opacity,
            position,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(ui, "缩放", "水印图片相对页面的宽度占比", |ui| {
                ui.add(egui::Slider::new(scale, 0.05..=1.0).fixed_decimals(2));
            });
            theme::field_row(ui, "透明度", |ui| {
                ui.add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2));
            });
            theme::field_row(ui, "位置", |ui| {
                ui.radio_value(position, WatermarkPlacement::Center, "居中");
                ui.radio_value(position, WatermarkPlacement::Tiled, "平铺");
            });
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "image_watermarked.pdf", &["pdf"], false);
    });
}
