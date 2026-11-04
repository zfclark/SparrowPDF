//! About dialog: what the app is, who made it, where the project lives.
//!
//! Rendered as a modal instead of a thirteenth tool page: the tool list is a
//! registry of PDF operations and "about" is shell chrome, so it must not take a
//! slot there. A modal also centres itself and keeps working at the 760x520
//! minimum window size, where a full page would have to scroll.
//!
//! No network code lives here. The project link is handed to
//! [`theme::open_url`], which asks the operating system to launch the default
//! browser; when that fails the dialog keeps the address copyable instead of
//! leaving a dead link.

use egui::{vec2, Align, Layout, RichText, TextStyle, Ui};

use super::theme::{self, GAP_M, GAP_S, GAP_XS};

/// Product facts, kept in one place so the dialog and the sidebar footer cannot
/// show different versions.
pub const APP_NAME: &str = "轻雀 PDF";
pub const APP_NAME_EN: &str = "SparrowPDF";
pub const TAGLINE: &str = "PDF 的事，本地办完。";
pub const AUTHOR: &str = "ZFClark（霖海）";
pub const PROJECT_URL: &str = "https://github.com/zfclark/SparrowPDF";
pub const LICENSE: &str = "MIT";
pub const COPYRIGHT_YEAR: &str = "2026";

/// Dialog state, owned by the app shell so the dialog survives repaints.
#[derive(Debug, Default)]
pub struct AboutState {
    /// Whether the dialog is on screen.
    pub open: bool,
    /// Last failure to reach the browser. While set, the dialog offers a
    /// "复制链接" fallback; clicking the link again retries and clears it.
    link_error: Option<String>,
}

impl AboutState {
    /// Put the dialog on screen, clearing a stale link error from an earlier
    /// attempt so the retry starts clean.
    pub fn show(&mut self) {
        self.open = true;
        self.link_error = None;
    }
}

/// Draw the dialog for this frame and update `state.open` when it is dismissed.
pub fn show(ctx: &egui::Context, state: &mut AboutState, icon: Option<&egui::TextureHandle>) {
    let screen = ctx.content_rect();
    // Fit the window instead of assuming a size: at the 760x520 minimum the
    // dialog must keep a visible backdrop on both sides, and on a large display
    // it should stay a dialog rather than stretch into a banner.
    let width = (screen.width() - 96.0).clamp(280.0, 420.0);
    // The rest of the dialog (header, footer, margins) needs roughly 190 pt, so
    // the scrolling body gets what is left — short screens scroll the sections
    // while the close button stays reachable.
    let body_height = (screen.height() - 250.0).max(120.0);

    // Window chrome from the active theme (fill, border, shadow), with a roomier
    // margin than a popup so the sections breathe.
    let style = ctx.style_of(ctx.theme());
    let frame = egui::Frame::window(&style)
        .inner_margin(egui::Margin::symmetric(18, 16))
        .corner_radius(egui::CornerRadius::same(12));

    let modal = egui::Modal::new(egui::Id::unique("sparrow_about"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            // The app has exactly one accent color; the link belongs to it
            // instead of egui's stock hyperlink blue.
            let accent = theme::accent(ui);
            ui.visuals_mut().hyperlink_color = accent;

            header(ui, icon);
            ui.add_space(GAP_M);
            divider(ui);

            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    basic_information(ui, ctx);
                    divider(ui);
                    author_information(ui);
                    divider(ui);
                    project_section(ui, state);
                });

            ui.add_space(GAP_M);
            divider(ui);
            footer(ui)
        });

    // Esc and a click on the backdrop both dismiss, which is what the shell
    // expects of a modal.
    if modal.inner || modal.should_close() {
        state.open = false;
    }
}

/// Icon, wordmark and tagline.
fn header(ui: &mut Ui, icon: Option<&egui::TextureHandle>) {
    ui.horizontal(|ui| {
        if let Some(texture) = icon {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(vec2(38.0, 38.0))
                    .corner_radius(egui::CornerRadius::same(9)),
            );
            ui.add_space(GAP_M);
        }
        ui.vertical(|ui| {
            ui.label(
                RichText::new(APP_NAME)
                    .strong()
                    .text_style(TextStyle::Heading),
            );
            ui.label(RichText::new(TAGLINE).small().weak());
        });
    });
}

/// Hairline between two sections, with breathing room on both sides.
fn divider(ui: &mut Ui) {
    ui.add_space(GAP_S);
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), theme::hairline(ui));
    ui.add_space(GAP_S);
}

/// Section caption, styled like the title inside [`theme::card`].
fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
    ui.add_space(GAP_XS);
}

/// One "label — value" line.
///
/// The label gets a fixed column so the values align, but the row height is
/// left to the content: a long value wraps to a second line on a narrow window
/// instead of overlapping the next row.
fn info_row(ui: &mut Ui, label: &str, value: &str) {
    info_row_sized(ui, label, value, false);
}

/// [`info_row`] with an optional smaller value, for long technical strings such
/// as file paths that would otherwise dominate the dialog.
fn info_row_sized(ui: &mut Ui, label: &str, value: &str, small: bool) {
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(theme::LABEL_W);
            let color = theme::hint_color(ui);
            ui.label(RichText::new(label).small().color(color));
        });
        let value = RichText::new(value);
        ui.label(if small { value.small() } else { value });
    });
}

/// 基本信息：身份 + 排查问题需要的环境信息。
///
/// A bug report is only actionable when the platform, the renderer's scale and
/// the log location are known, and those are exactly the facts a user cannot
/// read off the UI. Kept to four short rows so the dialog stays a glance.
fn basic_information(ui: &mut Ui, ctx: &egui::Context) {
    caption(ui, "基本信息");
    info_row(ui, "软件名称", &format!("{APP_NAME}（{APP_NAME_EN}）"));
    info_row(ui, "版本号", crate::version::display());
    info_row(ui, "运行平台", &platform_label());
    info_row(ui, "许可证", LICENSE);
    info_row(ui, "界面渲染", &renderer_label(ctx));
    let log_path = crate::engine::logging::log_path();
    info_row_sized(ui, "日志文件", &log_path.display().to_string(), true);
}

/// `Windows · x86_64` — the pair that decides which build a report is about.
fn platform_label() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    format!("{os} · {}", std::env::consts::ARCH)
}

/// Backend plus the current DPI scale: a wrong scale is the usual cause of
/// blurry text and clipped layouts.
fn renderer_label(ctx: &egui::Context) -> String {
    let scale = (ctx.pixels_per_point() * 100.0).round() as i32;
    format!("egui · Glow · 缩放 {scale}%")
}

/// 作者信息。
fn author_information(ui: &mut Ui) {
    caption(ui, "作者信息");
    info_row(ui, "作者", AUTHOR);
}

/// 项目地址：可点击链接，交给系统默认浏览器打开。
fn project_section(ui: &mut Ui, state: &mut AboutState) {
    caption(ui, "项目地址");
    let link = ui.add(egui::Link::new(PROJECT_URL));
    if link.clicked() {
        match theme::open_url(PROJECT_URL) {
            Ok(()) => {
                state.link_error = None;
                crate::engine::logging::log("INFO", "在系统浏览器中打开项目地址");
            }
            Err(err) => {
                crate::engine::logging::log("ERROR", &format!("打开项目地址失败：{err}"));
                state.link_error = Some(err.to_string());
            }
        }
    }

    // Only shown when the hand-off failed: the link would be dead on this
    // machine, so keep the address reachable another way.
    if let Some(err) = &state.link_error {
        ui.label(
            RichText::new(format!("注意：无法调用系统浏览器（{err}）"))
                .small()
                .color(theme::warning(ui)),
        );
        if theme::small_button(ui, "复制链接").clicked() {
            ui.ctx().copy_text(PROJECT_URL.to_string());
        }
    }
}

/// Copyright line on the left, close button on the right. Returns whether the
/// close button was pressed so the caller can dismiss the dialog.
fn footer(ui: &mut Ui) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        theme::hint(
            ui,
            &format!("© {COPYRIGHT_YEAR} ZFClark. All Rights Reserved."),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            clicked = theme::button(ui, "关闭").clicked();
        });
    });
    clicked
}
