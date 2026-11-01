//! Panel: PDF -> images.

use egui::Ui;

use crate::model::{ImageFormat, TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, multi_output_note,
    output_selector, password_field, section, status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::PdfToImage;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::PdfToImage { dpi, format } = form.options_mut(tool) {
        ui.horizontal(|ui| {
            ui.label("DPI");
            ui.add(egui::DragValue::new(dpi).range(36..=600).speed(1.0));
        });
        ui.horizontal(|ui| {
            ui.label("格式");
            ui.radio_value(format, ImageFormat::Png, "PNG");
            ui.radio_value(format, ImageFormat::Jpeg, "JPEG");
        });
    }
    ui.label(
        egui::RichText::new("使用纯 Rust 渲染，不依赖 PDFium/MuPDF；复杂矢量与透明效果可能有差异。")
            .small()
            .weak(),
    );
    password_field(ui, form);

    section(ui, "输出");
    multi_output_note(ui);
    output_selector(ui, form, "", &[], true);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
