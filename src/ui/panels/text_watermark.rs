//! Panel: text watermark.

use egui::Ui;

use crate::model::{ToolId, WatermarkPlacement};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::TextWatermark;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::TextWatermark {
            text,
            font_size,
            color,
            opacity,
            rotation,
            placement,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(
                ui,
                "水印文字",
                "中文水印使用系统中文字体渲染（如微软雅黑、思源黑体）",
                |ui| {
                    theme::text_input(ui, text, "请输入水印文字，例如：内部资料");
                },
            );
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(theme::LABEL_W, theme::ROW_H),
                    egui::Label::new("字号"),
                );
                theme::number_input(ui, font_size, 6.0..=200.0, |d| d.speed(1.0));
                ui.add_space(theme::GAP_S);
                ui.label("颜色");
                ui.color_edit_button_srgb(color);
            });
            theme::field_row(ui, "透明度", |ui| {
                ui.add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2));
            });
            theme::field_row(ui, "旋转角度", |ui| {
                theme::number_input(ui, rotation, -180.0..=180.0, |d| d.speed(1.0).suffix("°"));
            });
            theme::field_row(ui, "排布", |ui| {
                ui.radio_value(placement, WatermarkPlacement::Center, "居中");
                ui.radio_value(placement, WatermarkPlacement::Tiled, "平铺");
            });
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "watermarked.pdf", &["pdf"], false);
    });
}
