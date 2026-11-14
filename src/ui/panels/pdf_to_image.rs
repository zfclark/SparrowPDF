//! Panel: PDF -> images (one or more PDFs, one output folder).

use egui::Ui;

use crate::model::{ImageFormat, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    add_files_button, input_list, multi_output_note, output_selector_hinted, password_field,
    FormState,
};
use crate::ui::preview;
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState, preview_state: &mut preview::PreviewState) {
    let tool = ToolId::PdfToImage;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
        theme::hint(
            ui,
            "每个 PDF 逐页导出，文件名为「原名_p001」，可一次添加多个 PDF",
        );
    });

    let files: Vec<std::path::PathBuf> = form.inputs.iter().map(|f| f.path.clone()).collect();
    let password = (!form.password.is_empty()).then(|| form.password.clone());
    preview::show_pdf_pages(ui, &files, password.as_deref(), preview_state);

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::PdfToImage { dpi, format } = form.options_mut(tool) {
            theme::field_row_hover(
                ui,
                "DPI",
                "内置渲染器，复杂矢量与透明效果可能有差异",
                |ui| {
                    theme::number_input(ui, dpi, 36..=600, |d| d.speed(1.0).suffix(" DPI"));
                },
            );
            theme::hint(
                ui,
                "默认 300 DPI。提高分辨率可改善清晰度，同时增大体积与耗时。",
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
        output_selector_hinted(
            ui,
            form,
            "",
            &[],
            true,
            "未选择时导出至第一个 PDF 所在目录",
        );
    });
}
