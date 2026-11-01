//! Left sidebar: the 12 tools, grouped.
//!
//! Selection crossfades between rows instead of snapping, so switching tools
//! reads as motion rather than a jump.

use egui::{CornerRadius, Ui, Vec2, vec2};

use crate::model::{ToolGroup, ToolId};
use crate::ui::theme::{self, SIDEBAR_ROW_H, accent, hover_fill, lerp_color, painted_font, row_text_color};

/// Sidebar width in logical points; sized so 900x650 never scrolls.
const WIDTH: f32 = 204.0;

/// Draw the sidebar and return the tool the user selected this frame, if any.
pub fn show(ui: &mut Ui, current: ToolId, icon: Option<&egui::TextureHandle>) -> Option<ToolId> {
    let mut selected = None;

    egui::Panel::left("sparrow_sidebar")
        .resizable(false)
        .exact_size(WIDTH)
        .show(ui, |ui| {
            app_header(ui, icon);
            ui.add_space(theme::GAP_S);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for group in ToolGroup::ALL {
                        group_header(ui, group);
                        for tool in group.tools() {
                            if tool_row(ui, tool, tool == current) {
                                selected = Some(tool);
                            }
                        }
                        ui.add_space(theme::GAP_XS);
                    }
                });

            ui.add_space(theme::GAP_S);
            ui.painter().hline(
                ui.available_rect_before_wrap().x_range(),
                ui.available_rect_before_wrap().top(),
                theme::hairline(ui),
            );
            ui.add_space(theme::GAP_S);
            ui.label(egui::RichText::new("无上传 · 无遥测 · 本地处理").small().weak());
        });

    selected
}

fn app_header(ui: &mut Ui, icon: Option<&egui::TextureHandle>) {
    ui.add_space(theme::GAP_M);
    ui.horizontal(|ui| {
        if let Some(texture) = icon {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(vec2(30.0, 30.0))
                    .corner_radius(CornerRadius::same(7)),
            );
            ui.add_space(theme::GAP_S);
        }
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("轻雀 PDF")
                    .strong()
                    .text_style(egui::TextStyle::Heading),
            );
            ui.label(egui::RichText::new("PDF 的事，本地办完。").small().weak());
        });
    });
}

fn group_header(ui: &mut Ui, group: ToolGroup) {
    ui.add_space(theme::GAP_S);
    ui.label(egui::RichText::new(group.label()).small().weak());
    ui.add_space(2.0);
}

/// One tool entry: hand-painted so the selected state can animate.
fn tool_row(ui: &mut Ui, tool: ToolId, active: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, SIDEBAR_ROW_H), egui::Sense::click());

    // Crossfade the selection tint; egui drives this value toward the target.
    let t = ui
        .ctx()
        .animate_bool_responsive(ui.scope_id().with(("tool-row", tool)), active);

    if active || response.hovered() {
        let fill = if active {
            accent(ui).gamma_multiply(0.10 + 0.06 * t)
        } else {
            hover_fill(ui)
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::RADIUS_CTRL), fill);
    }

    // Leading indicator bar, fading in with the selection.
    if t > 0.01 {
        let bar = egui::Rect::from_min_size(
            rect.min + vec2(2.0, (rect.height() - 14.0) / 2.0),
            Vec2::new(3.0, 14.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), accent(ui).gamma_multiply(t));
    }

    let color = if active {
        lerp_color(row_text_color(ui), accent(ui), 0.35 + 0.65 * t)
    } else {
        row_text_color(ui)
    };
    let text_pos = rect.left_center() + vec2(14.0, 0.5);
    ui.painter().text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        tool.label(),
        painted_font(ui),
        color,
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}
