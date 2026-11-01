//! Panel: rotate pages.

use egui::Ui;

use crate::model::{RotationDegrees, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Rotate;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::Rotate { pages, degrees } = form.options_mut(tool) {
            theme::field_row_hover(ui, "页码范围", "留空或填 all 表示全部页面", |ui| {
                theme::text_input(ui, pages, "all=全部页面；或如 1-3,7");
            });
            theme::field_row_hover(
                ui,
                "旋转角度",
                "角度为增量：已有 90° 的页面再转 90° 会变成 180°",
                |ui| {
                    ui.radio_value(degrees, RotationDegrees::Deg90, "90°");
                    ui.radio_value(degrees, RotationDegrees::Deg180, "180°");
                    ui.radio_value(degrees, RotationDegrees::Deg270, "270°");
                },
            );
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "rotated.pdf", &["pdf"], false);
    });
}
