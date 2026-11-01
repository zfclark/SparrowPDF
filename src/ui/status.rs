//! Bottom bar: task state, result card, and the primary action.
//!
//! One sticky region below the content. The state signal (dot + label) sits on
//! the left of the action row; the result card appears above it only once a
//! task finished, so idle sessions keep the full window for content.

use egui::{RichText, Ui};

use crate::model::TaskState;
use crate::ui::panels::{PanelAction, PanelView};
use crate::ui::theme;

pub fn show(ui: &mut Ui, state: &TaskState, view: &PanelView) -> PanelAction {
    let mut action = PanelAction::None;

    egui::Panel::bottom("sparrow_bottom").show(ui, |ui| {
        if theme::result_area(ui, state) == theme::ResultAction::RetryInDocuments {
            action = PanelAction::RetryInDocuments;
        }

        // Hairline between the content above and the action row.
        let rect = ui.available_rect_before_wrap();
        ui.painter()
            .hline(rect.x_range(), rect.top(), theme::hairline(ui));
        ui.add_space(theme::GAP_S);

        ui.horizontal(|ui| {
            ui.add_space(theme::GAP_XS);
            theme::status_dot(ui, dot_color(ui, state));
            ui.add_space(theme::GAP_XS);
            ui.label(RichText::new(state_label(state)).strong());

            match state {
                TaskState::Running { progress, message } => {
                    ui.add_space(theme::GAP_S);
                    ui.add(
                        egui::ProgressBar::new(*progress)
                            .desired_width(120.0)
                            .show_percentage(),
                    );
                    ui.add_space(theme::GAP_XS);
                    ui.label(RichText::new(message).weak());
                }
                TaskState::Idle
                | TaskState::Validating
                | TaskState::Ready
                | TaskState::Done { .. }
                | TaskState::Failed { .. }
                | TaskState::Cancelled => {}
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = state.is_busy();
                let clickable = !busy && view.can_run;
                if theme::primary_button(ui, "执行", clickable).clicked() && clickable {
                    action = PanelAction::Run;
                }
                if busy {
                    if theme::button(ui, "取消").clicked() {
                        action = PanelAction::Cancel;
                    }
                    ui.add_space(theme::GAP_XS);
                    ui.spinner();
                } else if !view.can_run {
                    ui.label(RichText::new(view.hint).weak());
                }
            });
        });
        ui.add_space(theme::GAP_S);
    });

    action
}

fn state_label(state: &TaskState) -> &'static str {
    match state {
        TaskState::Idle => "空闲",
        TaskState::Validating => "校验中",
        TaskState::Ready => "就绪",
        TaskState::Running { .. } => "处理中",
        TaskState::Done { .. } => "完成",
        TaskState::Failed { .. } => "失败",
        TaskState::Cancelled => "已取消",
    }
}

fn dot_color(ui: &Ui, state: &TaskState) -> egui::Color32 {
    match state {
        TaskState::Idle | TaskState::Cancelled => ui.visuals().weak_text_color(),
        TaskState::Validating => theme::warning(ui),
        TaskState::Ready | TaskState::Running { .. } => theme::accent(ui),
        TaskState::Done { .. } => theme::success(ui),
        TaskState::Failed { .. } => theme::danger(ui),
    }
}
