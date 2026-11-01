//! Application shell: window setup, tool switching, task orchestration.

use std::path::PathBuf;

use eframe::egui;

use crate::engine::worker::{WorkerHandle, spawn};
use crate::model::{TaskState, ToolError, ToolId, WorkerMsg};
use crate::tools::ToolRequest;
use crate::ui::panels::{FormState, PanelAction, ReorderState};
use crate::ui::{panels, sidebar, status};

pub struct SparrowApp {
    current: ToolId,
    form: FormState,
    /// One task state per tool, so switching tools keeps each result visible.
    states: std::collections::BTreeMap<ToolId, TaskState>,
    worker: Option<(ToolId, WorkerHandle)>,
    reorder: ReorderState,
}

impl SparrowApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        install_cjk_fallback(&cc.egui_ctx);

        let mut states = std::collections::BTreeMap::new();
        for id in ToolId::ALL {
            states.insert(id, TaskState::Idle);
        }

        Self {
            current: ToolId::ImageToPdf,
            form: FormState::new(),
            states,
            worker: None,
            reorder: ReorderState::default(),
        }
    }

    fn state(&self) -> &TaskState {
        self.states.get(&self.current).unwrap_or(&TaskState::Idle)
    }

    /// Poll the running worker; returns true while a repaint is still needed.
    fn poll_worker(&mut self) -> bool {
        let Some((tool_id, handle)) = &self.worker else {
            return false;
        };
        let tool_id = *tool_id;
        let mut finished = None;
        let mut progress = None;

        while let Ok(msg) = handle.receiver.try_recv() {
            match msg {
                WorkerMsg::Progress { fraction, message } => {
                    progress = Some(TaskState::Running {
                        progress: fraction,
                        message,
                    });
                }
                WorkerMsg::Finished(result) => finished = Some(result),
            }
        }

        let mut had_progress = false;
        if let Some(state) = progress {
            had_progress = true;
            self.states.insert(tool_id, state);
        }

        match finished {
            Some(Ok(outcome)) => {
                self.states.insert(
                    tool_id,
                    TaskState::Done {
                        summary: outcome.summary,
                        outputs: outcome.outputs,
                    },
                );
                self.worker = None;
                true
            }
            Some(Err(ToolError::Cancelled)) => {
                self.states.insert(tool_id, TaskState::Cancelled);
                self.worker = None;
                true
            }
            Some(Err(err)) => {
                self.states.insert(tool_id, TaskState::Failed { error: err });
                self.worker = None;
                true
            }
            None => had_progress,
        }
    }

    fn cancel(&mut self) {
        if let Some((_, handle)) = &self.worker {
            handle.request_cancel();
        }
    }

    /// Translate the form into a request and start the worker.
    ///
    /// Runs the `Validating -> Ready -> Running` transitions of the state
    /// machine so the status bar shows each step, not just "processing".
    fn start(&mut self) {
        let tool_id = self.current;
        self.states.insert(tool_id, TaskState::Validating);

        let request = match self.build_request(tool_id) {
            Ok(request) => request,
            Err(err) => {
                self.states.insert(tool_id, TaskState::Failed { error: err });
                return;
            }
        };

        let tool = crate::tools::tool_for(tool_id);
        if let Err(err) = tool.validate(&request) {
            self.states.insert(tool_id, TaskState::Failed { error: err });
            return;
        }

        self.states.insert(tool_id, TaskState::Ready);
        self.states.insert(
            tool_id,
            TaskState::Running {
                progress: 0.0,
                message: "准备中".to_string(),
            },
        );
        self.worker = Some((tool_id, spawn(tool_id, request)));
    }

    /// Re-evaluate readiness for the active tool after the form changed.
    ///
    /// A tool that validates cleanly and has inputs moves from `Idle` to
    /// `Ready`; a failed pre-flight is reported immediately, before the user
    /// presses the button.
    fn refresh_readiness(&mut self) {
        if self.state().is_busy() || self.worker.is_some() {
            return;
        }
        if self.form.inputs.is_empty() {
            if matches!(self.state(), TaskState::Ready) {
                self.states.insert(self.current, TaskState::Idle);
            }
            return;
        }

        let tool_id = self.current;
        let Ok(request) = self.build_request(tool_id) else {
            return;
        };
        let tool = crate::tools::tool_for(tool_id);
        match tool.validate(&request) {
            Ok(()) => {
                if matches!(self.state(), TaskState::Idle | TaskState::Failed { .. }) {
                    self.states.insert(tool_id, TaskState::Ready);
                }
            }
            Err(_) => {
                // Incomplete options are normal while the user is typing, so an
                // invalid form simply stays `Idle` until something can run.
                if matches!(self.state(), TaskState::Ready) {
                    self.states.insert(tool_id, TaskState::Idle);
                }
            }
        }
    }

    fn build_request(&self, tool_id: ToolId) -> Result<ToolRequest, ToolError> {
        let inputs = self.form.inputs.clone();
        let output = self.resolve_output(tool_id);

        let password = if self.form.password.is_empty() {
            None
        } else {
            Some(self.form.password.clone())
        };

        Ok(ToolRequest {
            inputs,
            output,
            options: self.form.options_for(tool_id).clone(),
            password,
        })
    }

    /// Resolve the output path, defaulting to the source's directory.
    fn resolve_output(&self, tool_id: ToolId) -> PathBuf {
        if let Some(path) = &self.form.output {
            return path.clone();
        }

        let source = self
            .form
            .inputs
            .first()
            .map(|f| f.path.clone())
            .unwrap_or_else(|| PathBuf::from("output.pdf"));
        let parent = source
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        if tool_id.is_multi_output() {
            return parent;
        }

        let suffix = match tool_id {
            ToolId::ImageToPdf | ToolId::Merge => "_merged",
            ToolId::Rotate => "_rotated",
            ToolId::DeletePages => "_deleted",
            ToolId::Reorder => "_reordered",
            ToolId::Extract => "_extracted",
            ToolId::TextWatermark | ToolId::ImageWatermark => "_watermarked",
            ToolId::Encrypt => "_secured",
            _ => "_out",
        };

        let ext = match tool_id {
            ToolId::ExtractText => "txt",
            _ => "pdf",
        };
        parent.join(format!("{}{suffix}.{ext}", file_stem(&source)))
    }

    /// Accept files dropped onto the window, filtered by the active tool.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }

        let accepted = self.current.accepted_extensions();
        let filtered: Vec<PathBuf> = dropped
            .into_iter()
            .filter(|path| {
                path.extension()
                    .map(|e| {
                        let e = e.to_string_lossy().to_ascii_lowercase();
                        accepted.iter().any(|a| *a == e)
                    })
                    .unwrap_or(false)
            })
            .collect();

        if filtered.is_empty() {
            self.states.insert(
                self.current,
                TaskState::Failed {
                    error: ToolError::UnsupportedFormat {
                        path: None,
                        detail: format!(
                            "拖入的文件格式不被当前工具接受（{}）",
                            accepted.join(" / ")
                        ),
                    },
                },
            );
            return;
        }

        panels::append_inputs(self.current, &mut self.form, filtered);
        if matches!(self.states.get(&self.current), Some(TaskState::Failed { .. })) {
            self.states.insert(self.current, TaskState::Idle);
        }
    }

    fn show_panel(&mut self, ui: &mut egui::Ui) -> PanelAction {
        let state = self.states.get(&self.current).cloned().unwrap_or_default();
        let form = &mut self.form;

        match self.current {
            ToolId::ImageToPdf => panels::image_to_pdf::show(ui, form, &state),
            ToolId::Merge => panels::merge::show(ui, form, &state),
            ToolId::Split => panels::split::show(ui, form, &state),
            ToolId::Rotate => panels::rotate::show(ui, form, &state),
            ToolId::DeletePages => panels::delete_pages::show(ui, form, &state),
            ToolId::Reorder => panels::reorder::show(ui, form, &state, &mut self.reorder),
            ToolId::Extract => panels::extract::show(ui, form, &state),
            ToolId::TextWatermark => panels::text_watermark::show(ui, form, &state),
            ToolId::ImageWatermark => panels::image_watermark::show(ui, form, &state),
            ToolId::Encrypt => panels::encrypt::show(ui, form, &state),
            ToolId::PdfToImage => panels::pdf_to_image::show(ui, form, &state),
            ToolId::ExtractText => panels::extract_text::show(ui, form, &state),
        }
    }
}

impl eframe::App for SparrowApp {
    /// egui 0.37 hands the app a `Ui` per frame instead of the `Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

        if let Some(tool) = sidebar::show(ui, self.current) {
            if tool != self.current {
                self.current = tool;
                self.form.reset_inputs();
                self.reorder = ReorderState::default();
                self.states.insert(tool, TaskState::Idle);
            }
        }

        self.refresh_readiness();

        status::show(ui, self.state());

        let mut action = PanelAction::None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading(self.current.label());
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    action = self.show_panel(ui);
                });
        });

        match action {
            PanelAction::Run => self.start(),
            PanelAction::Cancel => self.cancel(),
            PanelAction::None => {}
        }

        if self.poll_worker() || self.state().is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string())
}

/// Window-wide spacing and font sizes tuned for the 900x650 default size.
fn configure_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 22.0;

        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(17.0, Proportional)),
            (TextStyle::Body, FontId::new(13.0, Proportional)),
            (TextStyle::Button, FontId::new(13.0, Proportional)),
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.0, egui::FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// Load a system CJK font at startup so Chinese labels render.
///
/// No font is embedded in the binary (that is what keeps the executable small);
/// when no system CJK font is found, readme documents the tofu fallback.
fn install_cjk_fallback(ctx: &egui::Context) {
    const CANDIDATES: &[(&str, &str, u32)] = &[
        ("msyh", r"C:\Windows\Fonts\msyh.ttc", 0),
        ("msyh-ttf", r"C:\Windows\Fonts\msyh.ttf", 0),
        ("dengxian", r"C:\Windows\Fonts\Deng.ttf", 0),
        ("simhei", r"C:\Windows\Fonts\simhei.ttf", 0),
        ("simsun", r"C:\Windows\Fonts\simsun.ttc", 0),
        ("pingfang", "/System/Library/Fonts/PingFang.ttc", 0),
        ("hiragino", "/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("noto-cjk", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ];

    for (name, path, index) in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let blob = std::sync::Arc::new(bytes);
        ctx.add_font(egui::FontInsert::new(
            name,
            egui::FontData::from_blob(blob, *index),
            vec![
                egui::InsertFontFamily {
                    family: egui::FontFamily::Proportional,
                    priority: egui::FontPriority::Lowest,
                },
                egui::InsertFontFamily {
                    family: egui::FontFamily::Monospace,
                    priority: egui::FontPriority::Lowest,
                },
            ],
        ));
        return;
    }
}
