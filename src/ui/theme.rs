//! Design tokens and shared chrome.
//!
//! Color, type, spacing and radius live here so every panel renders the same
//! visual language. Colors resolve against the active egui theme, so the app
//! looks right in both light and dark mode without a second code path.

use egui::{Color32, CornerRadius, FontId, FontFamily, Margin, Pos2, RichText, Stroke, TextStyle, Ui, vec2};

use crate::model::TaskState;

// ------------------------------------------------------------------ palette

/// The single accent color: selection, active state, primary action.
pub fn accent(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x0A, 0x84, 0xFF)
    } else {
        Color32::from_rgb(0x00, 0x7A, 0xFF)
    }
}

/// Success, as text / icon color (contrast-tuned per theme).
pub fn success(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x30, 0xD1, 0x58)
    } else {
        Color32::from_rgb(0x24, 0x8A, 0x3D)
    }
}

/// Warning, as text / icon color.
pub fn warning(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0xFF, 0xB3, 0x40)
    } else {
        Color32::from_rgb(0xB2, 0x5E, 0x00)
    }
}

/// Error, as text / icon color.
pub fn danger(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0xFF, 0x69, 0x61)
    } else {
        Color32::from_rgb(0xD7, 0x00, 0x15)
    }
}

/// Card surface: one step off the window background.
pub fn card_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(26)
    } else {
        Color32::WHITE
    }
}

/// Card border: a hairline, visible but quiet.
pub fn card_stroke(ui: &Ui) -> Stroke {
    if ui.visuals().dark_mode {
        Stroke::new(1.0, Color32::from_gray(52))
    } else {
        Stroke::new(1.0, Color32::from_rgb(0xE3, 0xE3, 0xE5))
    }
}

/// Row / list hover fill.
pub fn hover_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(40)
    } else {
        Color32::from_rgb(0xF0, 0xF0, 0xF2)
    }
}

/// Separator hairline between chrome regions.
pub fn hairline(ui: &Ui) -> Stroke {
    if ui.visuals().dark_mode {
        Stroke::new(1.0, Color32::from_gray(48))
    } else {
        Stroke::new(1.0, Color32::from_rgb(0xE8, 0xE8, 0xEA))
    }
}

/// Linear interpolation between two colors (used by animated chrome).
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
    )
}

// ------------------------------------------------------------------ metrics

pub const GAP_XS: f32 = 4.0;
pub const GAP_S: f32 = 8.0;
pub const GAP_M: f32 = 12.0;

/// Inner padding of a card.
pub const CARD_PAD: f32 = 12.0;
/// Corner radius of cards and the drop zone.
pub const RADIUS_CARD: u8 = 8;
/// Corner radius of buttons, rows and other controls.
pub const RADIUS_CTRL: u8 = 6;

/// Label column width in field rows, so all forms align.
pub const LABEL_W: f32 = 76.0;
/// Height of one form row.
pub const ROW_H: f32 = 26.0;
/// Height of standard buttons.
pub const CONTROL_H: f32 = 28.0;
/// Height of the primary action button.
pub const PRIMARY_H: f32 = 32.0;
/// Height of the empty-input drop zone.
pub const DROP_H: f32 = 64.0;
/// Standard width of text / password inputs.
pub const INPUT_W: f32 = 200.0;
/// Standard width of numeric (drag) inputs.
pub const NUMBER_W: f32 = 72.0;

// ---------------------------------------------------------------- components

/// Grouped section: rounded card with a quiet title.
pub fn card(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(card_fill(ui))
        .stroke(card_stroke(ui))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::symmetric(CARD_PAD as i8, 10))
        .outer_margin(Margin::symmetric(0, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(title).small().weak());
            ui.add_space(GAP_XS);
            add_contents(ui);
        });
}

/// Muted fill for the primary button while it cannot be pressed.
pub fn muted_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(46)
    } else {
        Color32::from_rgb(0xE9, 0xE9, 0xEB)
    }
}

/// The one accent-filled button per view.
///
/// `active` selects between the accent fill and a muted fill, because an
/// explicit fill would otherwise ignore egui's disabled visuals entirely.
pub fn primary_button(ui: &mut Ui, text: &str, active: bool) -> egui::Response {
    let (fill, fg) = if active {
        (accent(ui), Color32::WHITE)
    } else {
        (muted_fill(ui), ui.visuals().widgets.inactive.fg_stroke.color)
    };
    ui.add_sized(
        vec2(96.0, PRIMARY_H),
        egui::Button::new(RichText::new(text).color(fg).text_style(TextStyle::Button))
            .fill(fill)
            .corner_radius(CornerRadius::same(RADIUS_CTRL)),
    )
}

/// Standard button, rounded to match the primary one.
pub fn button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(text)
            .corner_radius(CornerRadius::same(RADIUS_CTRL))
            .min_size(vec2(0.0, CONTROL_H)),
    )
}

/// Small button for dense rows (e.g. list actions).
pub fn small_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).small().corner_radius(CornerRadius::same(RADIUS_CTRL - 1)))
}

/// Label + control on one line; labels share a fixed column so forms align.
pub fn field_row(ui: &mut Ui, label: &str, add_contents: impl FnOnce(&mut Ui)) {
    field_row_hover(ui, label, "", add_contents);
}

/// [`field_row`] with a tooltip on the label, for explanations that would
/// otherwise clutter the panel.
pub fn field_row_hover(ui: &mut Ui, label: &str, hover: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let label = ui.add_sized(
            vec2(LABEL_W, ROW_H),
            egui::Label::new(RichText::new(label)).truncate(),
        );
        if !hover.is_empty() {
            label.on_hover_text(hover);
        }
        add_contents(ui);
    });
}

/// Quiet supporting text, kept at a contrast that stays readable at a glance
/// (the default weak color is too faint to notice).
pub fn hint(ui: &mut Ui, text: &str) {
    let color = if ui.visuals().dark_mode {
        Color32::from_gray(150)
    } else {
        Color32::from_rgb(0x6E, 0x6E, 0x78)
    };
    ui.label(RichText::new(text).small().color(color));
}

// -------------------------------------------------------------------- inputs

/// Colors for input widgets: (background, resting border, focus border).
///
/// A slightly sunken background plus an explicit border is what makes an
/// editable field read as one, especially on white cards in light mode.
fn input_colors(dark: bool) -> (Color32, Color32, Color32) {
    if dark {
        (
            Color32::from_gray(18),
            Color32::from_gray(72),
            Color32::from_rgb(0x0A, 0x84, 0xFF),
        )
    } else {
        (
            Color32::from_rgb(0xF5, 0xF5, 0xF8),
            Color32::from_rgb(0xB4, 0xB4, 0xBC),
            Color32::from_rgb(0x00, 0x7A, 0xFF),
        )
    }
}

/// Placeholder text color: clearly present, clearly not user input.
fn placeholder_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(128)
    } else {
        Color32::from_rgb(0x9A, 0x9A, 0xA4)
    }
}

/// Style the current scope so every input widget added inside it gets the
/// visible border + sunken background treatment. Scoped (not global) so
/// buttons and cards keep their own look.
fn style_input_scope(ui: &mut Ui) {
    let (bg, border, border_focus) = input_colors(ui.visuals().dark_mode);
    let visuals = ui.visuals_mut();
    visuals.extreme_bg_color = bg;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.2, border_focus);
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, border_focus);
}

/// Clearly visible single-line input with an explicit placeholder.
///
/// Every panel routes text entry through here so inputs share one size and
/// one visual treatment.
pub fn text_input(ui: &mut Ui, text: &mut String, placeholder: &str) -> egui::Response {
    let hint_color = placeholder_color(ui);
    ui.scope(|ui| {
        style_input_scope(ui);
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(RichText::new(placeholder).color(hint_color))
                .desired_width(INPUT_W)
                .min_size(vec2(INPUT_W, CONTROL_H))
                .margin(Margin::symmetric(8, 0)),
        )
    })
    .inner
}

/// [`text_input`] with masked characters, for password fields.
pub fn password_input(ui: &mut Ui, text: &mut String, placeholder: &str) -> egui::Response {
    let hint_color = placeholder_color(ui);
    ui.scope(|ui| {
        style_input_scope(ui);
        ui.add(
            egui::TextEdit::singleline(text)
                .password(true)
                .hint_text(RichText::new(placeholder).color(hint_color))
                .desired_width(INPUT_W)
                .min_size(vec2(INPUT_W, CONTROL_H))
                .margin(Margin::symmetric(8, 0)),
        )
    })
    .inner
}

/// Clearly visible numeric input with a standard size. `customize` adds
/// per-field tuning (speed, suffix, …) on top of the shared look.
pub fn number_input<N: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    customize: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    ui.scope(|ui| {
        style_input_scope(ui);
        ui.add(
            customize(
                egui::DragValue::new(value)
                    .range(range)
                    .min_size(vec2(NUMBER_W, CONTROL_H)),
            ),
        )
    })
    .inner
}

/// Dashed target shown while the input list is empty.
pub fn drop_zone(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), DROP_H),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let color = ui.visuals().weak_text_color().gamma_multiply(0.7);
    dashed_rounded_rect(painter, rect, RADIUS_CARD as f32, color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::new(12.5, FontFamily::Proportional),
        color,
    );
}

/// Dashed rounded rectangle, drawn as a polyline so corners round properly.
fn dashed_rounded_rect(painter: &egui::Painter, rect: egui::Rect, radius: f32, color: Color32) {
    const FRAC_PI_2: f32 = std::f32::consts::FRAC_PI_2;
    const PI: f32 = std::f32::consts::PI;

    let r = rect.shrink(0.5);
    let radius = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    let step = 2.0;

    let mut path: Vec<Pos2> = Vec::new();
    let add_edge = |from: Pos2, to: Pos2, path: &mut Vec<Pos2>| {
        let len = (to - from).length();
        if len <= 0.0 {
            return;
        }
        let n = ((len / step).ceil() as usize).max(1);
        for i in 0..n {
            path.push(from + (to - from) * (i as f32 / n as f32));
        }
    };
    let add_corner = |center: Pos2, from: f32, to: f32, path: &mut Vec<Pos2>| {
        let arc_len = radius * (to - from).abs();
        let n = ((arc_len / step).ceil() as usize).max(1);
        for i in 0..n {
            let a = from + (to - from) * (i as f32 / n as f32);
            path.push(center + vec2(a.cos() * radius, a.sin() * radius));
        }
    };

    // Clockwise from the middle of the top edge.
    add_edge(
        Pos2::new(r.left() + radius, r.top()),
        Pos2::new(r.right() - radius, r.top()),
        &mut path,
    );
    add_corner(
        Pos2::new(r.right() - radius, r.top() + radius),
        -FRAC_PI_2,
        0.0,
        &mut path,
    );
    add_edge(
        Pos2::new(r.right(), r.top() + radius),
        Pos2::new(r.right(), r.bottom() - radius),
        &mut path,
    );
    add_corner(
        Pos2::new(r.right() - radius, r.bottom() - radius),
        0.0,
        FRAC_PI_2,
        &mut path,
    );
    add_edge(
        Pos2::new(r.right() - radius, r.bottom()),
        Pos2::new(r.left() + radius, r.bottom()),
        &mut path,
    );
    add_corner(
        Pos2::new(r.left() + radius, r.bottom() - radius),
        FRAC_PI_2,
        PI,
        &mut path,
    );
    add_edge(
        Pos2::new(r.left(), r.bottom() - radius),
        Pos2::new(r.left(), r.top() + radius),
        &mut path,
    );
    add_corner(
        Pos2::new(r.left() + radius, r.top() + radius),
        PI,
        3.0 * FRAC_PI_2,
        &mut path,
    );

    // Walk the polyline, alternating dash and gap.
    let (dash, gap): (f32, f32) = (5.0, 4.0);
    let mut drawing = true;
    let mut budget = dash;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let seg = (b - a).length();
        if seg <= 0.0 {
            continue;
        }
        let mut t = 0.0;
        while t < seg {
            let take = budget.min(seg - t);
            let p0 = a + (b - a) * (t / seg);
            let p1 = a + (b - a) * ((t + take) / seg);
            if drawing {
                painter.line_segment([p0, p1], Stroke::new(1.0, color));
            }
            budget -= take;
            t += take;
            if budget <= f32::EPSILON {
                drawing = !drawing;
                budget = if drawing { dash } else { gap };
            }
        }
    }
}

/// Colored dot for the status bar.
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center() + vec2(0.0, 0.5), 4.0, color);
}

/// Result area, shown once a task finished, failed or was cancelled.
///
/// Renders nothing while the tool is idle, so it costs no space until there
/// is something to report.
pub fn result_area(ui: &mut Ui, state: &TaskState) {
    match state {
        TaskState::Done { summary, outputs } => {
            tinted_card(ui, success(ui), |ui| {
                ui.label(RichText::new("已完成").strong().color(success(ui)));
                ui.label(summary);
                for output in outputs.iter().take(6) {
                    let link = ui
                        .link(RichText::new(output.display().to_string()).small())
                        .on_hover_text("点击打开");
                    if link.clicked() {
                        let _ = open_path(output);
                    }
                }
                if outputs.len() > 6 {
                    hint(ui, &format!("……另有 {} 个文件已输出", outputs.len() - 6));
                }
            });
        }
        TaskState::Failed { error } => {
            tinted_card(ui, danger(ui), |ui| {
                ui.label(RichText::new("处理失败").strong().color(danger(ui)));
                ui.label(RichText::new(error.to_string()).small());
            });
        }
        TaskState::Cancelled => {
            tinted_card(ui, ui.visuals().weak_text_color(), |ui| {
                ui.label(RichText::new("已取消").weak());
            });
        }
        TaskState::Idle | TaskState::Validating | TaskState::Ready | TaskState::Running { .. } => {}
    }
}

fn tinted_card(ui: &mut Ui, tint: Color32, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(tint.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, tint.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::symmetric(CARD_PAD as i8, 8))
        .outer_margin(Margin::symmetric(0, 5))
        .show(ui, add_contents);
}

/// Open a path with the platform default handler.
pub fn open_path(path: &std::path::Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// Row height shared by sidebar entries and list rows.
pub const SIDEBAR_ROW_H: f32 = 28.0;

/// Font used for manually painted text.
pub fn painted_font(ui: &Ui) -> FontId {
    ui.style()
        .text_styles
        .get(&TextStyle::Button)
        .cloned()
        .unwrap_or_else(|| FontId::new(13.0, FontFamily::Proportional))
}

/// Text color of an unselected row.
pub fn row_text_color(ui: &Ui) -> Color32 {
    ui.visuals().text_color()
}
