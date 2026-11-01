//! Bottom status bar: task state, progress percentage, result text.

use egui::{Color32, Ui};

use crate::model::TaskState;

/// Height reserved for the status bar.
pub const HEIGHT: f32 = 30.0;

pub fn show(ui: &mut Ui, state: &TaskState) {
    egui::Panel::bottom("sparrow_status")
        .exact_size(HEIGHT)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(4.0);

                let (color, label) = state_style(state);
                ui.label(egui::RichText::new(label).color(color));

                match state {
                    TaskState::Running { progress, message } => {
                        ui.add(
                            egui::ProgressBar::new(*progress)
                                .desired_width(140.0)
                                .show_percentage(),
                        );
                        ui.label(message);
                    }
                    TaskState::Done { summary, outputs } => {
                        ui.label(summary);
                        if !outputs.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("（{} 个文件）", outputs.len())).weak(),
                            );
                        }
                    }
                    TaskState::Failed { error } => {
                        ui.label(egui::RichText::new(error.to_string()).color(Color32::LIGHT_RED));
                    }
                    TaskState::Idle | TaskState::Validating | TaskState::Ready => {
                        ui.label(egui::RichText::new("选择工具并添加文件后开始").weak());
                    }
                    TaskState::Cancelled => {
                        ui.label(egui::RichText::new("任务已取消").weak());
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if let Some(progress) = state.progress() {
                        // Plain text next to the bar, so the percentage is not
                        // conveyed by the painted bar alone.
                        ui.label(format!("进度 {}%", (progress * 100.0).round() as i32));
                    }
                });
            });
        });
}

fn state_style(state: &TaskState) -> (Color32, &'static str) {
    match state {
        TaskState::Idle => (Color32::GRAY, "空闲"),
        TaskState::Validating => (Color32::YELLOW, "校验中"),
        TaskState::Ready => (Color32::LIGHT_BLUE, "就绪"),
        TaskState::Running { .. } => (Color32::LIGHT_BLUE, "处理中"),
        TaskState::Done { .. } => (Color32::LIGHT_GREEN, "完成"),
        TaskState::Failed { .. } => (Color32::LIGHT_RED, "失败"),
        TaskState::Cancelled => (Color32::GRAY, "已取消"),
    }
}
