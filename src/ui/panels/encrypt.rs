//! Panel: encrypt / decrypt.

use egui::Ui;

use crate::model::{TaskState, ToolId};
use crate::tools::ToolOptions;
use crate::ui::panels::{
    FormState, PanelAction, action_row, add_files_button, input_list, output_selector, section,
    status_area,
};

pub fn show(ui: &mut Ui, form: &mut FormState, state: &TaskState) -> PanelAction {
    let tool = ToolId::Encrypt;

    section(ui, "输入 PDF");
    add_files_button(ui, tool, form);
    input_list(ui, tool, &mut form.inputs, false);

    section(ui, "选项");
    if let ToolOptions::Encrypt {
        encrypt,
        user_password,
        owner_password,
        allow_print,
        allow_copy,
        allow_modify,
    } = form.options_mut(tool)
    {
        ui.horizontal(|ui| {
            ui.radio_value(encrypt, true, "加密");
            ui.radio_value(encrypt, false, "解密");
        });

        if *encrypt {
            ui.horizontal(|ui| {
                ui.label("用户密码");
                ui.add(
                    egui::TextEdit::singleline(user_password)
                        .password(true)
                        .desired_width(160.0)
                        .hint_text("打开文件时需要"),
                );
            });
            ui.horizontal(|ui| {
                ui.label("所有者密码");
                ui.add(
                    egui::TextEdit::singleline(owner_password)
                        .password(true)
                        .desired_width(160.0)
                        .hint_text("修改权限时需要"),
                );
            });
            ui.label(egui::RichText::new("算法：AES-256").small().weak());
            ui.horizontal(|ui| {
                ui.label("允许");
                ui.checkbox(allow_print, "打印");
                ui.checkbox(allow_copy, "复制");
                ui.checkbox(allow_modify, "修改");
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("当前密码");
                ui.add(
                    egui::TextEdit::singleline(&mut form.password)
                        .password(true)
                        .desired_width(160.0),
                );
            });
            ui.label(
                egui::RichText::new("解密将移除加密，输出文件可直接打开。")
                    .small()
                    .weak(),
            );
        }
    }

    section(ui, "输出");
    let default_name = match form.options_for(tool) {
        ToolOptions::Encrypt { encrypt: true, .. } => "encrypted.pdf",
        _ => "decrypted.pdf",
    };
    output_selector(ui, form, default_name, &["pdf"], false);

    section(ui, "状态");
    status_area(ui, state);

    let can_run = !form.inputs.is_empty();
    action_row(ui, state, can_run, "请先选择一个 PDF 文件。")
}
