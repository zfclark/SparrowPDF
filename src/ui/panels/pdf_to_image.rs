//! Panel: PDF -> images.

use egui::Ui;

use crate::model::{ImageFormat, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, multi_output_note, output_selector,
    password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::PdfToImage;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::PdfToImage { dpi, format } = form.options_mut(tool) {
            theme::field_row_hover(
                ui,
                "DPI",
                "纯 Rust 软件渲染，不依赖 PDFium / MuPDF；复杂矢量与透明效果可能有差异",
                |ui| {
                    theme::number_input(ui, dpi, 36..=600, |d| d.speed(1.0).suffix(" DPI"));
                },
            );
            theme::field_row(ui, "格式", |ui| {
                ui.radio_value(format, ImageFormat::Png, "PNG");
                ui.radio_value(format, ImageFormat::Jpeg, "JPEG");
            });
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        multi_output_note(ui);
        output_selector(ui, form, "", &[], true);
    });
}
