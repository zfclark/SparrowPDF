//! Panel: page replacement.
//!
//! Two clearly separated slots (target vs. source) because mixing them up is
//! the one mistake this tool must not allow: the target is only read, the source
//! is what lands on the page. The source slot accepts a PDF or a photo, and the
//! hints spell out how each one behaves.

use std::path::PathBuf;

use egui::{RichText, Ui};

use crate::model::{InputFile, ToolId};
use crate::tools::replace_pages::is_pdf;
use crate::tools::{ReplaceFit, ReplaceGeometry, ToolOptions};
use crate::ui::panels::{output_selector, password_field, FormState};
use crate::ui::preview::{self, PreviewState};
use crate::ui::theme;

/// The two file slots, in the order they are stored in `FormState::inputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `inputs[0]` — the PDF whose pages are replaced (read only).
    Target,
    /// `inputs[1]` — the PDF page or photo that takes over.
    Source,
}

impl Slot {
    fn index(self) -> usize {
        match self {
            Slot::Target => 0,
            Slot::Source => 1,
        }
    }

    fn button_label(self) -> &'static str {
        match self {
            Slot::Target => "选择被替换的 PDF…",
            Slot::Source => "选择来源文件…",
        }
    }

    fn filter_name(self) -> &'static str {
        match self {
            Slot::Target => "PDF",
            Slot::Source => "PDF 与图片",
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Slot::Target => &["pdf"],
            Slot::Source => &["pdf", "jpg", "jpeg", "png"],
        }
    }
}

pub fn show(ui: &mut Ui, form: &mut FormState, preview: &mut PreviewState) {
    let tool = ToolId::ReplacePages;

    let target_is_pdf = form.inputs.first().is_some_and(|file| is_pdf(&file.path));
    let source_is_pdf = form.inputs.get(1).is_some_and(|file| is_pdf(&file.path));
    let has_source = form.inputs.len() > 1;

    // A photo has no page box, so "follow the source size" would have no
    // meaning. Switching the source from a PDF to a photo therefore falls back
    // to keeping the target sheet instead of leaving an impossible choice
    // selected behind a disabled radio.
    if has_source && !source_is_pdf {
        if let ToolOptions::ReplacePages { geometry, .. } = form.options_mut(tool) {
            if *geometry == ReplaceGeometry::FollowSource {
                *geometry = ReplaceGeometry::KeepTarget;
            }
        }
    }

    theme::card(ui, "① 目标文件（被替换的 PDF）", |ui| {
        slot(ui, form, Slot::Target, "未选择");
        theme::hint(
            ui,
            "只读取这个文件；结果为另存的新 PDF，原文件不会被改动。",
        );
    });

    theme::card(ui, "② 替换来源（PDF 或照片 / 图片）", |ui| {
        slot(
            ui,
            form,
            Slot::Source,
            if target_is_pdf {
                "未选择"
            } else {
                "请先在上方选择目标 PDF"
            },
        );
        ui.horizontal_wrapped(|ui| {
            if has_source && theme::small_button(ui, "互换目标与来源").clicked() {
                form.inputs.swap(0, 1);
            }
            theme::hint(
                ui,
                "PDF 按页取内容（文字仍可选中）；JPG / PNG 作为整页内容铺到目标页上。",
            );
        });
    });

    theme::card(ui, "替换范围", |ui| {
        if let ToolOptions::ReplacePages {
            pages,
            source_pages,
            fit,
            geometry,
        } = form.options_mut(tool)
        {
            theme::field_row_hover(
                ui,
                "目标页",
                "被替换的页码，从 1 开始；例如 3 或 2-4,7",
                |ui| {
                    theme::text_input(ui, pages, "例如 2-4,7");
                },
            );

            ui.add_enabled_ui(source_is_pdf, |ui| {
                theme::field_row_hover(
                    ui,
                    "来源页",
                    "留空＝重复使用第 1 页；填多页时数量需与目标页一致",
                    |ui| {
                        theme::text_input(ui, source_pages, "留空 = 第 1 页");
                    },
                );
            });

            theme::field_row_hover(ui, "页面适配", "等比完整显示会留白；铺满会裁掉超出部分", |ui| {
                for option in ReplaceFit::ALL {
                    ui.radio_value(fit, option, option.label());
                }
            });

            ui.add_enabled_ui(source_is_pdf, |ui| {
                theme::field_row_hover(
                    ui,
                    "替换方式",
                    "「保持原页尺寸」只换内容；「跟随来源页面尺寸」连纸张一起换",
                    |ui| {
                        ui.radio_value(
                            geometry,
                            ReplaceGeometry::KeepTarget,
                            ReplaceGeometry::KeepTarget.label(),
                        );
                        ui.radio_value(
                            geometry,
                            ReplaceGeometry::FollowSource,
                            ReplaceGeometry::FollowSource.label(),
                        );
                    },
                );
            });

            if has_source && !source_is_pdf {
                theme::hint(
                    ui,
                    "图片没有页面尺寸：按「保持原页尺寸」处理，图片按上面的适配方式放进目标页。",
                );
            }

            password_field(ui, form);
        }
    });

    theme::card(ui, "输出", |ui| {
        output_selector(ui, form, "replaced.pdf", &["pdf"], false);
    });

    let target = form.inputs.first().map(|file| file.path.clone());
    let source = form.inputs.get(1).map(|file| file.path.clone());
    let (pages_spec, source_spec) = match form.options_for(tool) {
        ToolOptions::ReplacePages {
            pages,
            source_pages,
            ..
        } => (pages.clone(), source_pages.clone()),
        _ => (String::new(), String::new()),
    };
    let password = if form.password.is_empty() {
        None
    } else {
        Some(form.password.clone())
    };

    preview::show_replacement(
        ui,
        target.as_deref(),
        source.as_deref(),
        &pages_spec,
        &source_spec,
        password.as_deref(),
        preview,
    );
}

/// One file slot: picker, current file, role tag and a clear button.
///
/// The source slot stays disabled until a target exists, so the two files can
/// never be stored in the wrong order by picking the source first.
fn slot(ui: &mut Ui, form: &mut FormState, which: Slot, empty_hint: &str) {
    let index = which.index();
    let enabled = match which {
        Slot::Target => true,
        Slot::Source => form.inputs.first().is_some_and(|file| is_pdf(&file.path)),
    };

    let mut picked: Option<PathBuf> = None;
    let mut cleared = false;

    ui.horizontal(|ui| {
        let mut button = ui
            .add_enabled_ui(enabled, |ui| theme::button(ui, which.button_label()))
            .inner;
        if !enabled {
            button = button.on_hover_text("请先选择目标 PDF：第 1 个文件才是被替换的对象");
        }
        if button.clicked() {
            picked = rfd::FileDialog::new()
                .add_filter(which.filter_name(), which.extensions())
                .pick_file();
        }

        match form.inputs.get(index) {
            Some(file) => {
                ui.add(egui::Label::new(RichText::new(file.display_name())).truncate())
                    .on_hover_text(file.path.display().to_string());
                let tag = match (which, is_pdf(&file.path)) {
                    (Slot::Target, _) => "PDF",
                    (Slot::Source, true) => "PDF 来源",
                    (Slot::Source, false) => "图片来源",
                };
                ui.label(
                    RichText::new(tag)
                        .small()
                        .color(theme::hint_color(ui)),
                );
                if theme::icon_button(ui, "×", "移除这个文件").clicked() {
                    cleared = true;
                }
            }
            None => {
                theme::hint(ui, empty_hint);
            }
        }
    });

    if let Some(path) = picked {
        let file = InputFile::new(path);
        match index {
            0 if form.inputs.is_empty() => form.inputs.push(file),
            0 => form.inputs[0] = file,
            1 if form.inputs.len() < 2 => form.inputs.push(file),
            _ => form.inputs[1] = file,
        }
    }
    if cleared && index < form.inputs.len() {
        form.inputs.remove(index);
    }
}
