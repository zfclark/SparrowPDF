//! Panel: text watermark.

use egui::Ui;

use crate::model::{ToolId, WatermarkPlacement};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

/// Frequently used watermark tints, shown as one-click swatches next to the
/// color picker: black, gray, red, orange, blue, green.
const PRESET_COLORS: &[(&str, [u8; 3])] = &[
    ("黑色", [17, 17, 17]),
    ("灰色", [128, 128, 128]),
    ("红色", [200, 30, 30]),
    ("橙色", [230, 126, 34]),
    ("蓝色", [0, 90, 200]),
    ("绿色", [30, 140, 60]),
];

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
                // Quick presets: one click sets the tint without opening the
                // color picker. Rendered as small filled squares.
                for preset in PRESET_COLORS {
                    let (name, rgb) = *preset;
                    let button = egui::Button::new("")
                        .fill(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
                        .min_size(egui::vec2(18.0, 18.0))
                        .corner_radius(egui::CornerRadius::same(4))
                        .stroke(ui.visuals().widgets.inactive.bg_stroke);
                    if ui.add(button).on_hover_text(name).clicked() {
                        *color = rgb;
                    }
                }
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
