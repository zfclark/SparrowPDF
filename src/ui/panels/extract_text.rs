//! Panel: text extraction.

use egui::Ui;

use crate::model::ToolId;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::ExtractText;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        theme::hint(ui, "仅提取文本层；扫描件无文本。输出 UTF-8 纯文本。");
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "extracted.txt", &["txt"], false);
    });
}
