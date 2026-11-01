//! Panel: delete pages.

use egui::Ui;

use crate::model::ToolId;
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, output_selector, password_field,
};
use crate::ui::preview;
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState, preview_state: &mut preview::PreviewState) {
    let tool = ToolId::DeletePages;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    let files: Vec<std::path::PathBuf> = form.inputs.iter().map(|f| f.path.clone()).collect();
    preview::show_pdf_pages(ui, &files, preview_state);

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::DeletePages { pages } = form.options_mut(tool) {
            theme::field_row_hover(ui, "删除页码", "如 2,5-7；不能删除全部页面", |ui| {
                theme::text_input(ui, pages, "例如：2,5-7");
            });
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "deleted.pdf", &["pdf"], false);
    });
}
