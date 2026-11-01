//! Panel: folder batch conversion (folders -> PDFs).

use egui::Ui;

use crate::model::ToolId;
use crate::tools::{BatchPageMode, Orientation, ToolOptions};
use crate::ui::panels::{
    FormState, add_folders_button, input_list_hinted, output_selector_hinted,
};
use crate::ui::preview;
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState, preview_state: &mut preview::PreviewState) {
    let tool = ToolId::FolderToPdf;

    theme::card(ui, "输入文件夹", |ui| {
        add_folders_button(ui, tool, form);
        input_list_hinted(
            ui,
            tool,
            &mut form.inputs,
            true,
            "拖放文件夹到此处，或点击「添加文件夹…」",
        );
        theme::hint(ui, "每个文件夹合成一个 PDF；图片按文件名中的数字排序（1、2、…、10），无数字的按系统顺序排在最后");
    });

    let folders: Vec<std::path::PathBuf> = form.inputs.iter().map(|f| f.path.clone()).collect();
    preview::show_folders(ui, &folders, preview_state);

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::FolderToPdf {
            mode,
            force_orientation,
            merge_spec,
            auto_rotate,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(
                ui,
                "每页布局",
                "每页一张：每张图片占一页；两图一页：两张图片并排占一个横向页面",
                |ui| {
                    ui.radio_value(mode, BatchPageMode::Single, "每页一张");
                    ui.radio_value(mode, BatchPageMode::TwoPerPage, "两图一页");
                },
            );
            match mode {
                BatchPageMode::Single => {
                    theme::field_row_hover(
                        ui,
                        "页面方向",
                        "按图片：每张图片各自决定页面方向；A4 竖版 / A4 横版：统一页面方向",
                        |ui| {
                            ui.radio_value(force_orientation, None, "按图片");
                            ui.radio_value(force_orientation, Some(Orientation::Portrait), "A4 竖版");
                            ui.radio_value(force_orientation, Some(Orientation::Landscape), "A4 横版");
                        },
                    );
                    theme::field_row_hover(
                        ui,
                        "自动旋转",
                        "勾选后，横向图片放入竖版页面（或纵向图片放入横版页面）时自动旋转 90° 铺满页面",
                        |ui| {
                            ui.checkbox(auto_rotate, "方向不匹配时旋转 90°");
                        },
                    );
                }
                BatchPageMode::TwoPerPage => {
                    theme::field_row_hover(
                        ui,
                        "自动旋转",
                        "两图一页使用横向 A4；横向图片放入竖向半页时自动旋转 90°",
                        |ui| {
                            ui.checkbox(auto_rotate, "方向不匹配时旋转 90°");
                        },
                    );
                    theme::field_row_hover(
                        ui,
                        "合并序号",
                        "留空 = 按顺序每两张一页；也可指定如 1-2,5-6，未提到的图片单独成页（序号见上方预览）",
                        |ui| {
                            theme::text_input(ui, merge_spec, "如 1-2,5-6，留空则按顺序两两合并");
                        },
                    );
                }
            }
        }
    });

    theme::card(ui, "输出", |ui| {
        output_selector_hinted(
            ui,
            form,
            "",
            &[],
            true,
            "未选择，默认保存到各文件夹内（文件名与文件夹同名）",
        );
    });
}
