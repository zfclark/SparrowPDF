//! Panel: PDF compression.
//!
//! One file, one strength choice: the tool itself decides what is safe to
//! re-encode, so the panel keeps the promise text concrete (what changes, what
//! never does) instead of exposing technical knobs.

use egui::Ui;

use crate::model::ToolId;
use crate::tools::{CompressLevel, ToolOptions};
use crate::ui::panels::{add_files_button, input_list, output_selector, FormState};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Compress;

    theme::card(ui, "输入", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "压缩强度", |ui| {
        if let ToolOptions::Compress { level } = form.options_mut(tool) {
            theme::field_row(ui, "强度", |ui| {
                for option in CompressLevel::ALL {
                    ui.radio_value(level, option, option.label());
                }
            });
            theme::hint(ui, level.detail());
            theme::hint(
                ui,
                "只重新编码图片并清理未引用对象：页面顺序、文字、矢量与字体保持不变。加密的 PDF 请先移除密码。",
            );
        }
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "compressed.pdf", &["pdf"], false);
    });
}
