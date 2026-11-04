//! "The target file already exists" dialog.
//!
//! The tool layer never overwrites on its own: it returns
//! [`crate::model::ToolError::OutputExists`] with every clashing path, and this
//! modal turns that into a decision. Batch runs can hit many collisions at
//! once, so the dialog answers for the whole run and offers to remember the
//! choice for the rest of the session ("本次后续均执行此操作").
//!
//! A modal instead of a banner: the choice blocks the task, and a stale banner
//! could be scrolled away while the worker is already gone.

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Ui};

use crate::model::ToolId;
use crate::ui::theme::{self, GAP_M, GAP_S, GAP_XS};

/// How many clashing paths are listed before collapsing into "另有 N 个".
const LIST_LIMIT: usize = 4;

/// What the user picked in the dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Still open, nothing picked.
    None,
    /// Abandon the task.
    Cancel,
    /// Replace the existing files.
    Overwrite { remember: bool },
    /// Keep the existing files, write the new ones under `-2`, `-3`, …
    Unique { remember: bool },
}

/// Dialog state, owned by the app shell so it survives repaints.
#[derive(Debug, Default)]
pub struct ConflictState {
    /// Whether the dialog is on screen.
    pub open: bool,
    /// Tool whose run produced the clash; the retry goes back to it.
    pub tool: Option<ToolId>,
    /// Paths that already exist, exactly as the tool planned them.
    pub paths: Vec<PathBuf>,
    /// "本次后续均执行此操作" checkbox.
    remember: bool,
}

impl ConflictState {
    /// Show the dialog for `paths` produced by `tool`.
    pub fn ask(&mut self, tool: ToolId, paths: Vec<PathBuf>) {
        self.open = true;
        self.tool = Some(tool);
        self.paths = paths;
        self.remember = false;
    }

    /// Hide the dialog and forget the run it belonged to.
    pub fn close(&mut self) {
        self.open = false;
        self.tool = None;
        self.paths.clear();
        self.remember = false;
    }
}

/// Draw the dialog for this frame and report the user's decision.
pub fn show(ctx: &egui::Context, state: &mut ConflictState) -> ConflictChoice {
    let mut choice = ConflictChoice::None;
    if !state.open || state.paths.is_empty() {
        return choice;
    }

    let screen = ctx.content_rect();
    let width = (screen.width() - 96.0).clamp(320.0, 520.0);
    let style = ctx.style_of(ctx.theme());
    let frame = egui::Frame::window(&style)
        .inner_margin(egui::Margin::symmetric(18, 16))
        .corner_radius(egui::CornerRadius::same(12));

    let modal = egui::Modal::new(egui::Id::unique("sparrow_conflict"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);

            ui.label(
                RichText::new("目标文件已存在")
                    .strong()
                    .color(theme::warning(ui))
                    .text_style(egui::TextStyle::Heading),
            );
            ui.add_space(GAP_S);

            let count = state.paths.len();
            let lead = if count == 1 {
                "已经存在同名文件，继续会替换它：".to_string()
            } else {
                format!("已经存在 {count} 个同名文件，继续会替换它们：")
            };
            ui.label(lead);
            ui.add_space(GAP_XS);

            // A batch run writes into one folder, so show that folder once and
            // list only the names. Joining "dir\name" by hand was wrong on
            // macOS and doubled the separator on Windows.
            let mut parents: Vec<&std::path::Path> =
                state.paths.iter().filter_map(|p| p.parent()).collect();
            parents.sort();
            parents.dedup();
            let single_dir = (parents.len() == 1).then(|| parents[0]);
            if let Some(dir) = single_dir {
                theme::hint(ui, &dir.display().to_string());
            }

            ui.indent("conflict-paths", |ui| {
                for path in state.paths.iter().take(LIST_LIMIT) {
                    let text = match single_dir {
                        Some(_) => path.file_name().map_or_else(
                            || path.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        ),
                        None => path.display().to_string(),
                    };
                    ui.add(
                        egui::Label::new(RichText::new(text).small().weak())
                            .truncate()
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text(path.display().to_string());
                }
            });
            if count > LIST_LIMIT {
                theme::hint(ui, &format!("……另有 {} 个文件", count - LIST_LIMIT));
            }

            ui.add_space(GAP_S);
            theme::hint(ui, "覆盖无法撤销；选择「另存为新文件」会保留原文件。");
            ui.add_space(GAP_XS);
            ui.checkbox(
                &mut state.remember,
                RichText::new("本次后续均执行此操作").small(),
            )
            .on_hover_text("同一会话内，后续出现的同名文件不再询问，直接沿用本次选择");
            ui.add_space(GAP_M);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Safer choice on the primary button: keeping both files is
                // never destructive.
                if theme::primary_button(ui, "另存为新文件", true).clicked() {
                    choice = ConflictChoice::Unique {
                        remember: state.remember,
                    };
                }
                if overwrite_button(ui).clicked() {
                    choice = ConflictChoice::Overwrite {
                        remember: state.remember,
                    };
                }
                if theme::button(ui, "取消").clicked() {
                    choice = ConflictChoice::Cancel;
                }
            });
        });

    if modal.should_close() {
        // Esc / click outside counts as cancel: doing nothing is the safe
        // answer, and the task is already stopped at this point.
        choice = ConflictChoice::Cancel;
    }

    choice
}

/// Destructive action: same shape as a normal button, danger-colored text.
fn overwrite_button(ui: &mut Ui) -> egui::Response {
    let danger = theme::danger(ui);
    ui.add(
        egui::Button::new(RichText::new("覆盖").color(danger))
            .min_size(egui::vec2(80.0, theme::PRIMARY_H))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS_CTRL)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dialog has to survive a frame on a bare context (no window, no CJK
    /// font) and must stay open until the user answers.
    #[test]
    fn dialog_renders_and_waits_for_an_answer() {
        let ctx = egui::Context::default();
        let mut state = ConflictState::default();
        state.ask(
            ToolId::Merge,
            vec![
                PathBuf::from("C:/out/merged.pdf"),
                PathBuf::from("C:/out/merged-2.pdf"),
            ],
        );

        let mut choice = ConflictChoice::None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            choice = show(ui.ctx(), &mut state);
        });

        assert_eq!(choice, ConflictChoice::None, "no click, no decision");
        assert!(state.open, "the dialog stays until answered");
        assert_eq!(state.paths.len(), 2, "both clashing paths are kept");
    }

    #[test]
    fn closing_clears_the_run() {
        let mut state = ConflictState::default();
        state.ask(ToolId::Rotate, vec![PathBuf::from("a.pdf")]);
        state.close();
        assert!(!state.open);
        assert!(state.paths.is_empty());
        assert!(state.tool.is_none());
    }
}
