//! Left sidebar: the 12 tools, grouped.

use egui::{Ui, vec2};

use crate::model::{ToolGroup, ToolId};

/// Sidebar width in logical points; sized so 900x650 never scrolls.
const WIDTH: f32 = 200.0;
/// Row height for one tool entry.
const ROW_HEIGHT: f32 = 26.0;

/// Draw the sidebar and return the tool the user selected this frame, if any.
pub fn show(ui: &mut Ui, current: ToolId) -> Option<ToolId> {
    let mut selected = None;

    egui::Panel::left("sparrow_sidebar")
        .resizable(false)
        .exact_size(WIDTH)
        .show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("轻雀 PDF").heading());
            ui.label(egui::RichText::new("PDF 的事，本地办完。").small().weak());
            ui.add_space(8.0);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for group in ToolGroup::ALL {
                        group_header(ui, group);
                        for tool in group.tools() {
                            if tool_button(ui, tool, tool == current) {
                                selected = Some(tool);
                            }
                        }
                        ui.add_space(4.0);
                    }
                });
        });

    selected
}

fn group_header(ui: &mut Ui, group: ToolGroup) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(group.label()).small().weak());
}

fn tool_button(ui: &mut Ui, tool: ToolId, active: bool) -> bool {
    ui.add_sized(
        vec2(ui.available_width(), ROW_HEIGHT),
        egui::Button::new(tool.label()).selected(active),
    )
    .clicked()
}
