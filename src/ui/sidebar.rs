//! Left sidebar: the 12 tools, grouped.
//!
//! Selection crossfades between rows instead of snapping, so switching tools
//! reads as motion rather than a jump.

use egui::{vec2, Color32, CornerRadius, FontFamily, FontId, Stroke, Ui, Vec2};

use crate::model::{ToolGroup, ToolId};
use crate::ui::theme::{
    self, accent, hover_fill, lerp_color, painted_font, row_text_color, SIDEBAR_ROW_H,
};

/// Sidebar width in logical points; sized so 900x650 never scrolls.
const WIDTH: f32 = 204.0;

/// Height of the About entry in the footer.
///
/// 26 pt keeps it shorter than a tool row: it is shell chrome, not a tool, and
/// the footer has to keep all 12 tools above the fold at the default window.
const ABOUT_ROW_H: f32 = 26.0;

/// What the sidebar produced this frame.
#[derive(Debug, Default)]
pub struct SidebarOutput {
    /// Tool the user selected, if any.
    pub selected: Option<ToolId>,
    /// Appearance the user asked for, when the footer switch was used.
    pub theme: Option<bool>,
    /// The About entry was pressed.
    pub about: bool,
}

/// Draw the sidebar and report the tool / appearance the user picked.
pub fn show(
    ui: &mut Ui,
    current: ToolId,
    icon: Option<&egui::TextureHandle>,
    dark: bool,
) -> SidebarOutput {
    let mut output = SidebarOutput::default();

    egui::Panel::left("sparrow_sidebar")
        .resizable(false)
        .exact_size(WIDTH)
        .show(ui, |ui| {
            // The footer is declared first, as a bottom panel: it reserves its
            // own space so the tool list (which fills the rest) can never push
            // it out of view.
            egui::Panel::bottom("sparrow_sidebar_footer").show(ui, |ui| {
                footer(ui, dark, &mut output);
            });

            app_header(ui, icon);
            ui.add_space(theme::GAP_XS);

            // Tight vertical rhythm. With the 6 pt default gap the list needed
            // ~700 pt and only 10 of the 12 tools fit at the default 900x650
            // window: 安全 (加密与解密) and 转换 (PDF 转图片) sat below the fold
            // with no visible scroll bar, so the app looked like it shipped ten
            // tools. Every row and caption now fits at the default size.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for group in ToolGroup::ALL {
                            group_header(ui, group);
                            for tool in group.tools() {
                                if tool_row(ui, tool, tool == current) {
                                    output.selected = Some(tool);
                                }
                            }
                            ui.add_space(theme::GAP_XS);
                        }
                    });
            });
        });

    output
}

/// Sidebar footer: appearance switch and the About entry.
fn footer(ui: &mut Ui, dark: bool, output: &mut SidebarOutput) {
    ui.add_space(theme::GAP_XS);
    ui.painter().hline(
        ui.available_rect_before_wrap().x_range(),
        ui.available_rect_before_wrap().top(),
        theme::hairline(ui),
    );
    ui.add_space(theme::GAP_S);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("外观").small().weak());
        if let Some(dark) = theme::theme_switch(ui, dark) {
            output.theme = Some(dark);
        }
    });
    ui.add_space(theme::GAP_S);
    if about_row(ui) {
        output.about = true;
    }
    ui.add_space(theme::GAP_XS);
}

/// Footer entry that opens the About dialog.
///
/// Painted rather than built from a stock button so the version can sit on the
/// right edge: a `Button` centres a single label, and the version would then
/// only be discoverable after opening the dialog. The border and hover fill
/// make it read as a button, not as the static text above it.
fn about_row(ui: &mut Ui) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ABOUT_ROW_H), egui::Sense::click());

    let hovered = response.hovered();
    let (fill, stroke) = if hovered {
        (
            hover_fill(ui),
            Stroke::new(1.0, accent(ui).gamma_multiply(0.45)),
        )
    } else {
        (Color32::TRANSPARENT, theme::card_stroke(ui))
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::RADIUS_CTRL),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );

    let color = if hovered {
        accent(ui)
    } else {
        row_text_color(ui)
    };
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.5),
        egui::Align2::LEFT_CENTER,
        "关于",
        FontId::new(12.5, FontFamily::Proportional),
        color,
    );
    // The full five-position version does not fit next to a longer label, so the
    // entry is labelled "关于" and the version takes the right edge: it is the
    // first thing support asks for.
    ui.painter().text(
        rect.right_center() + vec2(-10.0, 0.5),
        egui::Align2::RIGHT_CENTER,
        crate::version::display(),
        FontId::new(10.5, FontFamily::Proportional),
        ui.visuals().weak_text_color(),
    );

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// App badge: icon plus wordmark and tagline, on one row.
fn app_header(ui: &mut Ui, icon: Option<&egui::TextureHandle>) {
    ui.add_space(theme::GAP_S);
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
    ui.add_space(8.0);
    ui.label(egui::RichText::new(group.label()).small().weak());
}

/// One tool entry: hand-painted so the selected state can animate.
fn tool_row(ui: &mut Ui, tool: ToolId, active: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, SIDEBAR_ROW_H), egui::Sense::click());

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
