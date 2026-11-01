//! Panel: extract pages.

use egui::Ui;

use crate::model::ToolId;
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Extract;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::Extract { pages } = form.options_mut(tool) {
            theme::field_row_hover(ui, "提取页码", "如 1-3,7；抽取的页面组成新文件", |ui| {
                theme::text_input(ui, pages, "例如：1-3,7");
            });
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "extracted.pdf", &["pdf"], false);
    });
}
