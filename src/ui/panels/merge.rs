//! Panel: PDF merge.

use egui::Ui;

use crate::model::ToolId;
use crate::ui::panels::{FormState, add_files_button, input_list, output_selector, password_field};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Merge;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, true);
    });

    theme::card(ui, "选项", |ui| {
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "merged.pdf", &["pdf"], false);
    });
}
