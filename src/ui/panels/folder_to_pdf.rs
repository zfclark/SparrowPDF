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
    let numbering = match form.options_for(tool) {
        ToolOptions::FolderToPdf {
            mode,
            use_merge_spec,
            merge_spec,
            ..
        } => Some(preview::PageNumbering {
            two_per_page: *mode == BatchPageMode::TwoPerPage,
            use_merge_spec: *use_merge_spec,
            merge_spec: merge_spec.clone(),
        }),
        _ => None,
    };
    preview::show_folders(ui, &folders, numbering.as_ref(), preview_state);

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::FolderToPdf {
            mode,
            force_orientation,
            pair_orientation,
            use_merge_spec,
            merge_spec,
            auto_rotate,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(
                ui,
                "每页布局",
                "每页一张：每张图片占一页；两图一页：两张图片并排占一个页面",
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
                        "页面方向",
                        "A4 横版：两图左右并排；A4 竖版：两图上下并排",
                        |ui| {
                            ui.radio_value(pair_orientation, Orientation::Landscape, "A4 横版");
                            ui.radio_value(pair_orientation, Orientation::Portrait, "A4 竖版");
                        },
                    );
                    theme::field_row_hover(
                        ui,
                        "自动旋转",
                        "勾选后，图片方向与半页方向不一致时自动旋转 90° 铺满",
                        |ui| {
                            ui.checkbox(auto_rotate, "方向不匹配时旋转 90°");
                        },
                    );
                    theme::field_row_hover(
                        ui,
                        "指定合并",
                        "勾选：仅对下方输入的序号执行两两合并，其余图片单独成页；不勾选：按顺序对所有图片两两合并",
                        |ui| {
                            ui.checkbox(use_merge_spec, "指定合并序号");
                        },
                    );
                    if *use_merge_spec {
                        theme::field_row_hover(
                            ui,
                            "合并序号",
                            "格式如 1-2,5-6；序号为图片在文件夹中的编号（悬停预览缩略图可见），输出页面按原顺序连续编号",
                            |ui| {
                                theme::text_input(ui, merge_spec, "如 1-2,5-6");
                            },
                        );
                        theme::hint(ui, "勾选后未填写序号、格式非法或序号超出图片数量时，任务会失败并给出具体原因");
                    }
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
