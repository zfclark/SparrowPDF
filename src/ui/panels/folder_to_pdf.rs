//! Panel: folder batch conversion (folders -> PDFs).
//!
//! The output format comes from the template table (`tools::BATCH_TEMPLATES`);
//! this panel never hard-codes one. A new template shows up here — dropdown
//! entry, format text and applied values — without touching this file.

use egui::{RichText, Ui};

use crate::model::ToolId;
use crate::tools::{
    BatchPageMode, Orientation, PageSize, ScanQuality, ToolOptions, BATCH_TEMPLATES,
};
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
        theme::hint(
            ui,
            "每个文件夹生成一个 PDF。文件名中的数字按数值排序（1、2、…、10），无数字者排在末尾。",
        );
    });

    // Template first: the preview labels below depend on the layout it sets.
    template_card(ui, form, tool);
    params_card(ui, form, tool);

    let folders: Vec<std::path::PathBuf> = form.inputs.iter().map(|f| f.path.clone()).collect();
    let numbering = match form.options_for(tool) {
        ToolOptions::FolderToPdf {
            template,
            mode,
            use_merge_spec,
            merge_spec,
            ..
        } => Some(preview::PageNumbering {
            two_per_page: *mode == BatchPageMode::TwoPerPage,
            use_merge_spec: *use_merge_spec,
            merge_spec: merge_spec.clone(),
            strict_merge_spec: !template.owns_merge_spec(merge_spec),
        }),
        _ => None,
    };
    preview::show_folders(ui, &folders, numbering.as_ref(), preview_state);

    theme::card(ui, "输出", |ui| {
        output_selector_hinted(ui, form, "", &[], true, "未选择时存入各自文件夹，文件名为文件夹名");
    });
}

/// Template picker plus the format requirement of the selection.
fn template_card(ui: &mut Ui, form: &mut FormState, tool: ToolId) {
    let current = match form.options_for(tool) {
        ToolOptions::FolderToPdf { template, .. } => *template,
        _ => return,
    };
    // The dropdown edits a copy; applying it rewrites the option set, so it
    // happens after the card closes (the borrow of `form` is gone by then).
    let mut chosen = current;

    theme::card(ui, "模板", |ui| {
        theme::field_row_hover(
            ui,
            "模板",
            "按材料类型选择，纸张、方向与每页张数由模板确定",
            |ui| {
                egui::ComboBox::from_id_salt(("batch-template", tool))
                    .width(230.0)
                    .selected_text(chosen.label)
                    .show_ui(ui, |ui| {
                        for candidate in BATCH_TEMPLATES {
                            ui.selectable_value(&mut chosen, candidate, candidate.label);
                        }
                    });
            },
        );
        ui.add_space(theme::GAP_XS);
        ui.label(RichText::new("格式要求").small().weak());
        ui.label(
            RichText::new(chosen.format)
                .small()
                .color(theme::hint_color(ui)),
        );
    });

    if chosen.key != current.key {
        form.options_mut(tool).apply_batch_template(chosen);
    }
}

/// Parameter card. Rows are shown only when the template lets the user change
/// them, so a named template shows just its own knobs and nothing greyed out.
fn params_card(ui: &mut Ui, form: &mut FormState, tool: ToolId) {
    theme::card(ui, "排版参数", |ui| {
        let ToolOptions::FolderToPdf {
            template,
            mode,
            force_orientation,
            pair_orientation,
            page_size,
            use_merge_spec,
            merge_spec,
            auto_rotate,
            scan_quality,
            ..
        } = form.options_mut(tool)
        else {
            return;
        };
        let adjust = template.adjust;
        let preset = &template.preset;

        if adjust.page_size {
            theme::field_row_hover(ui, "页面大小", "纸张尺寸，图片按比例缩放至页面内", |ui| {
                egui::ComboBox::from_id_salt(("batch-page-size", tool))
                    .width(200.0)
                    .selected_text(page_size.label())
                    .show_ui(ui, |ui| {
                        for candidate in PageSize::ALL {
                            ui.selectable_value(page_size, candidate, candidate.label());
                        }
                    });
            });
        }

        if adjust.page_layout {
            let mut layout = *mode;
            theme::field_row_hover(
                ui,
                "每页布局",
                "每页一张：单张图片独占一页；两图一页：两张图片共用一个页面",
                |ui| {
                    ui.radio_value(&mut layout, BatchPageMode::Single, "每页一张");
                    ui.radio_value(&mut layout, BatchPageMode::TwoPerPage, "两图一页");
                },
            );
            if layout != *mode {
                *mode = layout;
                // A merged page needs one direction for the whole sheet; keep
                // the direction already in use instead of jumping to a default.
                if layout == BatchPageMode::TwoPerPage {
                    *pair_orientation = force_orientation.unwrap_or(*pair_orientation);
                }
            }
        }

        // Recomputed after the layout switch so the rows below already follow
        // the new layout in this frame.
        let two_per_page = *mode == BatchPageMode::TwoPerPage;

        if adjust.page_orientation {
            let current = if two_per_page {
                Some(*pair_orientation)
            } else {
                *force_orientation
            };
            let mut direction = current;
            theme::field_row_hover(
                ui,
                "页面方向",
                "按图片：按每张图片的方向设置；竖版 / 横版：全文统一（两图一页时上下或左右并排）",
                |ui| {
                    if !two_per_page {
                        ui.radio_value(&mut direction, None, "按图片");
                    }
                    ui.radio_value(&mut direction, Some(Orientation::Portrait), "竖版");
                    ui.radio_value(&mut direction, Some(Orientation::Landscape), "横版");
                },
            );
            if direction != current {
                match direction {
                    Some(chosen) => {
                        *force_orientation = Some(chosen);
                        if two_per_page {
                            *pair_orientation = chosen;
                        }
                    }
                    // Only reachable for single pages; a pair page keeps a
                    // direction.
                    None => *force_orientation = None,
                }
            }
        }

        let merge_row = adjust.merge && (two_per_page || preset.merge_spec.is_some());
        if merge_row {
            let mut merged = *use_merge_spec;
            theme::field_row_hover(
                ui,
                "页合并",
                "勾选后仅合并下列页码，其余图片单独成页",
                |ui| {
                    ui.checkbox(&mut merged, "合并到同一页");
                },
            );
            if merged != *use_merge_spec {
                *use_merge_spec = merged;
                // A template that exists for one merge falls back to one image
                // per page when the switch goes off.
                if preset.merge_spec.is_some() {
                    *mode = if merged {
                        BatchPageMode::TwoPerPage
                    } else {
                        BatchPageMode::Single
                    };
                }
            }
        }

        // The page field only exists while merging is on, and only when the
        // user owns the numbers (入团志愿书 fixes 2–3 in the preset).
        if merge_row && *use_merge_spec && preset.merge_spec.is_none() {
            theme::field_row_hover(
                ui,
                "页码",
                "图片在文件夹中的序号（悬停预览可见），如 2-3",
                |ui| {
                    theme::text_input(ui, merge_spec, "如 2-3");
                },
            );
        }

        if adjust.auto_rotate {
            theme::field_row_hover(
                ui,
                "自动旋转",
                "图片方向与页面或栏位不一致时旋转 90° 铺满",
                |ui| {
                    ui.checkbox(auto_rotate, "方向不符时转 90°");
                },
            );
        }

        // Print quality is not part of a layout, so every template offers it:
        // a 600 DPI scan of an A4 sheet carries 4× the pixels the page can
        // show, and that surplus is pure file size.
        theme::field_row_hover(
            ui,
            "体积优化",
            "扫描件按打印所需分辨率重新压缩；单个 PDF 超过 20 MB 时会自动再压一档",
            |ui| {
                egui::ComboBox::from_id_salt(("batch-scan-quality", tool))
                    .width(230.0)
                    .selected_text(scan_quality.label())
                    .show_ui(ui, |ui| {
                        for candidate in ScanQuality::ALL {
                            ui.selectable_value(scan_quality, candidate, candidate.label());
                        }
                    });
            },
        );
    });
}
