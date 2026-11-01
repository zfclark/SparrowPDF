//! Panel: split.

use egui::Ui;

use crate::model::{PageRangeMode, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, add_files_button, input_list, multi_output_note, output_selector,
    password_field,
};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Split;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::Split {
            mode,
            spec,
            every_n,
        } = form.options_mut(tool)
        {
            theme::field_row(ui, "拆分方式", |ui| {
                ui.radio_value(mode, PageRangeMode::Explicit, "按页码范围");
                ui.radio_value(mode, PageRangeMode::EveryN, "按每 N 页");
                ui.radio_value(mode, PageRangeMode::SinglePages, "按单页");
            });
            match mode {
                PageRangeMode::Explicit => {
                    theme::field_row_hover(
                        ui,
                        "页码范围",
                        "如 1-3,7；分段输出为一个文件",
                        |ui| {
                            theme::text_input(ui, spec, "例如：1-3,7 或 5");
                        },
                    );
                }
                PageRangeMode::EveryN => {
                    theme::field_row(ui, "每 N 页", |ui| {
                        theme::number_input(ui, every_n, 1..=9999, |d| d.speed(1.0));
                        ui.label("页为一个文件");
                    });
                }
                PageRangeMode::SinglePages => {
                    theme::hint(ui, "每一页输出一个 PDF 文件");
                }
            }
        }
        password_field(ui, form);
    });

    theme::card(ui, "输出", |ui| {
        multi_output_note(ui);
        output_selector(ui, form, "", &[], true);
    });
}
