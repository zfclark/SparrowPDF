//! Panel: images -> PDF.

use egui::Ui;

use crate::model::ToolId;
use crate::tools::{Orientation, ToolOptions};
use crate::ui::panels::{FormState, add_files_button, input_list, output_selector};
use crate::ui::preview;
use crate::ui::theme;

pub fn show(ui: &mut Ui, form: &mut FormState, preview_state: &mut preview::PreviewState) {
    let tool = ToolId::ImageToPdf;

    theme::card(ui, "输入图片", |ui| {
        add_files_button(ui, tool, form);
        input_list(ui, tool, &mut form.inputs, true);
    });

    let files: Vec<std::path::PathBuf> = form.inputs.iter().map(|f| f.path.clone()).collect();
    preview::show_files(ui, &files, preview_state);

    theme::card(ui, "选项", |ui| {
        if let ToolOptions::ImageToPdf {
            force_orientation,
            auto_rotate,
        } = form.options_mut(tool)
        {
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
                "勾选后，横向图片放入竖版页面（或纵向图片放入横版页面）时自动旋转 90° 铺满页面，不再缩小居中",
                |ui| {
                    ui.checkbox(auto_rotate, "方向不匹配时旋转 90°");
                },
            );
            theme::hint(ui, "例：横向图片 + A4 竖版 + 自动旋转 = 图片竖向铺满整页");
        }
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "merged.pdf", &["pdf"], false);
    });
}
