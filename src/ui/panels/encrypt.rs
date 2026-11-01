//! Panel: encrypt / decrypt.

use egui::Ui;

use crate::model::ToolId;
use crate::tools::ToolOptions;
use crate::ui::panels::{FormState, add_files_button, input_list, output_selector};
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState) {
    let tool = ToolId::Encrypt;

    theme::card(ui, "输入 PDF", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, false);
    });

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::Encrypt {
            encrypt,
            user_password,
            owner_password,
            allow_print,
            allow_copy,
            allow_modify,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(
                ui,
                "操作",
                "AES-256 加密：用户密码用于打开文件，所有者密码用于修改权限",
                |ui| {
                    ui.radio_value(encrypt, true, "加密");
                    ui.radio_value(encrypt, false, "解密");
                },
            );

            if *encrypt {
                theme::field_row_hover(ui, "用户密码", "打开文件时需要输入", |ui| {
                    theme::password_input(ui, user_password, "设置打开文件所需的密码");
                });
                theme::field_row_hover(ui, "所有者密码", "修改权限或解密时需要", |ui| {
                    theme::password_input(ui, owner_password, "用于修改权限，可留空");
                });
                theme::field_row(ui, "允许", |ui| {
                    ui.checkbox(allow_print, "打印");
                    ui.checkbox(allow_copy, "复制");
                    ui.checkbox(allow_modify, "修改");
                });
            } else {
                theme::field_row(ui, "当前密码", |ui| {
                    theme::password_input(ui, &mut form.password, "输入当前文件的打开密码");
                });
                theme::hint(ui, "解密将移除加密，输出文件可直接打开");
            }
        }
    });

    theme::card(ui, "输出", |ui| {
        let default_name = match form.options_for(tool) {
            ToolOptions::Encrypt { encrypt: true, .. } => "encrypted.pdf",
            _ => "decrypted.pdf",
        };
        output_selector(ui, form, default_name, &["pdf"], false);
    });
}
