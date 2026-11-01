//! Application shell: window setup, tool switching, task orchestration.

use std::path::PathBuf;

use eframe::egui;

use crate::engine::worker::{WorkerHandle, spawn};
use crate::model::{TaskState, ToolError, ToolId, WorkerMsg};
use crate::tools::ToolRequest;
use crate::ui::panels::{FormState, PanelAction, ReorderState, panel_view};
use crate::ui::{panels, sidebar, status, theme};

/// App icon, embedded at compile time from the shipped launcher icon.
///
/// `assets/icon.png` is the design reference and must stay untouched; the
/// runtime icon uses the generated launcher asset instead.
pub(crate) const ICON_PNG: &[u8] = include_bytes!("../assets/ic_launcher_512.png");

pub struct SparrowApp {
    current: ToolId,
    form: FormState,
    /// One task state per tool, so switching tools keeps each result visible.
    states: std::collections::BTreeMap<ToolId, TaskState>,
    worker: Option<(ToolId, WorkerHandle)>,
    reorder: ReorderState,
    /// App icon, decoded once from the bytes already embedded for the window.
    icon: Option<egui::TextureHandle>,
    /// TEMP verification hook; removed after the visual check.
    probe: Probe,
    /// Cached directory-writability probe results, so the fallback check in
    /// `resolve_output` never touches the filesystem more than once per
    /// directory (it runs on every readiness refresh otherwise).
    writable_cache: std::collections::BTreeMap<PathBuf, bool>,
    /// Chosen fallback directory per unwritable source directory, so the
    /// decision (and its log line) happens exactly once.
    fallback_cache: std::collections::BTreeMap<PathBuf, PathBuf>,
    /// Note to attach to the next finished task (e.g. "output was redirected").
    task_note: Option<String>,
    /// Thumbnail preview state shared by the image→PDF panels.
    preview: crate::ui::preview::PreviewState,
    /// Saved appearance choice; `None` follows the operating-system theme.
    theme_pref: Option<crate::ui::prefs::ThemePref>,
}

/// TEMP: environment-driven screenshot probe for visual verification.
struct Probe {
    out: Option<String>,
    files: Vec<PathBuf>,
    dark: bool,
    done: bool,
    failed: bool,
    frames: u64,
}

impl SparrowApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        install_cjk_fallback(&cc.egui_ctx);

        // TEMP probe setup (removed after the visual check).
        let probe = Probe {
            out: std::env::var("SPARROW_SHOT").ok(),
            files: std::env::var("SPARROW_SHOT_FILES")
                .ok()
                .map(|v| v.split(';').map(PathBuf::from).collect())
                .unwrap_or_default(),
            dark: std::env::var("SPARROW_SHOT_DARK").is_ok(),
            done: std::env::var("SPARROW_SHOT_DONE").is_ok(),
            failed: std::env::var("SPARROW_SHOT_FAIL").is_ok(),
            frames: 0,
        };
        // Appearance: the screenshot probe wins, then the saved user choice.
        // With neither, leave egui alone so the app keeps following the
        // operating-system theme (pinning a guess here would freeze the first
        // frame into the wrong mode).
        let theme_pref = probe
            .dark
            .then_some(crate::ui::prefs::ThemePref::Dark)
            .or_else(crate::ui::prefs::load);
        if let Some(pref) = theme_pref {
            cc.egui_ctx.set_theme(if pref.is_dark() {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
        }
        let mut current = ToolId::ImageToPdf;
        if let Ok(label) = std::env::var("SPARROW_SHOT_TOOL") {
            if let Some(tool) = ToolId::ALL.into_iter().find(|t| t.label() == label) {
                current = tool;
            }
        }
        let mut form = FormState::new();
        for path in &probe.files {
            form.inputs.push(crate::model::InputFile::new(path.clone()));
        }
        // TEMP probe option presets, for screenshotting specific UI states.
        if std::env::var("SPARROW_SHOT_TWOPAGE").is_ok() {
            if let crate::tools::ToolOptions::FolderToPdf { mode, .. } = form.options_mut(current) {
                *mode = crate::tools::BatchPageMode::TwoPerPage;
            }
        }
        if std::env::var("SPARROW_SHOT_PAIRPORTRAIT").is_ok() {
            if let crate::tools::ToolOptions::FolderToPdf {
                pair_orientation, ..
            } = form.options_mut(current)
            {
                *pair_orientation = crate::tools::Orientation::Portrait;
            }
        }
        if let Ok(spec) = std::env::var("SPARROW_SHOT_MERGESPEC") {
            if let crate::tools::ToolOptions::FolderToPdf {
                use_merge_spec,
                merge_spec,
                ..
            } = form.options_mut(current)
            {
                *use_merge_spec = true;
                *merge_spec = spec;
            }
        }
        let mut states = std::collections::BTreeMap::new();
        for id in ToolId::ALL {
            states.insert(id, TaskState::Idle);
        }
        if probe.done {
            states.insert(
                current,
                TaskState::Done {
                    summary: "已合并 2 个文件，共 5 页".to_string(),
                    outputs: vec![
                        PathBuf::from("E:\\output\\merged.pdf"),
                        PathBuf::from("E:\\output\\merged-2.pdf"),
                    ],
                    note: Some("源文件夹不可写，输出已改存到「文档\\SparrowPDF」".to_string()),
                },
            );
        }
        if probe.failed {
            states.insert(
                current,
                TaskState::Failed {
                    error: crate::model::ToolError::ReadFailed {
                        path: PathBuf::from("E:\\broken.pdf"),
                        detail: "文件已损坏或不是有效的 PDF".to_string(),
                    },
                },
            );
        }

        Self {
            current,
            form,
            states,
            worker: None,
            reorder: ReorderState::default(),
            icon: load_icon(&cc.egui_ctx),
            probe,
            writable_cache: std::collections::BTreeMap::new(),
            fallback_cache: std::collections::BTreeMap::new(),
            task_note: None,
            preview: crate::ui::preview::PreviewState::new(),
            theme_pref,
        }
    }

    /// Apply the appearance choice, remember it, and log the switch.
    fn set_theme_pref(&mut self, ctx: &egui::Context, pref: crate::ui::prefs::ThemePref) {
        if self.theme_pref == Some(pref) {
            return;
        }
        self.theme_pref = Some(pref);
        ctx.set_theme(if pref.is_dark() {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        crate::ui::prefs::save(pref);
        crate::engine::logging::log(
            "INFO",
            if pref.is_dark() {
                "界面外观切换为深色"
            } else {
                "界面外观切换为浅色"
            },
        );
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
                        note: self.task_note.take(),
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

        // Fail fast when the chosen output location cannot be written, so the
        // user sees a specific message immediately instead of a failure after
        // processing.
        if let Err(err) = preflight_output(tool_id, &request) {
            crate::engine::logging::log(
                "ERROR",
                &format!("输出位置预检失败：{err}"),
            );
            self.states.insert(tool_id, TaskState::Failed { error: err });
            return;
        }

        crate::engine::logging::log(
            "INFO",
            &format!(
                "开始处理（{tool_id}）：输入 {} 项，输出 {}",
                request.inputs.len(),
                request.output.display()
            ),
        );

        // Remember whether the default output had to be redirected, so the
        // result card can tell the user where the file actually went.
        self.task_note = self.redirect_note(tool_id, &request);

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

    /// Build the "output was redirected" note for the result card.
    ///
    /// Only applies to automatically resolved destinations: an explicitly
    /// chosen path is never silently moved.
    fn redirect_note(
        &self,
        tool_id: ToolId,
        request: &crate::tools::ToolRequest,
    ) -> Option<String> {
        if self.form.output.is_some() || tool_id.is_multi_output() {
            return None;
        }
        if request.output.as_os_str().is_empty() {
            return None;
        }
        let source_parent = self
            .form
            .inputs
            .first()
            .and_then(|f| f.path.parent())
            .map(|p| p.to_path_buf());
        let out_parent = request.output.parent().map(|p| p.to_path_buf());
        if source_parent.is_some() && source_parent != out_parent {
            out_parent.map(|dir| format!("源文件夹不可写，输出已改存到「{}」", dir.display()))
        } else {
            None
        }
    }

    /// Retry the current task with its output redirected into the writable
    /// fallback folder (offered by the failure card).
    fn retry_in_documents(&mut self) {
        let Some(dir) = fallback_output_dir() else {
            crate::engine::logging::log("ERROR", "没有找到可写的回退目录");
            return;
        };
        let tool = self.current;
        let target = if tool.is_multi_output() {
            dir.clone()
        } else {
            let source = self
                .form
                .inputs
                .first()
                .map(|f| f.path.clone())
                .unwrap_or_else(|| PathBuf::from("output.pdf"));
            dir.join(format!("{}{}.pdf", file_stem(&source), output_suffix(tool)))
        };
        crate::engine::logging::log(
            "INFO",
            &format!("按用户选择改存到：{}", target.display()),
        );
        self.form.output = Some(target);
        self.start();
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

    fn build_request(&mut self, tool_id: ToolId) -> Result<ToolRequest, ToolError> {
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
    ///
    /// When that directory refuses writes (protected folders such as
    /// `C:\Program Files`, or a read-only drive, which a non-admin process
    /// cannot save into), fall back to the first writable user folder so the
    /// output still lands somewhere the user can open.
    fn resolve_output(&mut self, tool_id: ToolId) -> PathBuf {
        if let Some(path) = &self.form.output {
            return path.clone();
        }

        // Folder batch conversion defaults to writing each PDF into its own
        // folder (the tool interprets an empty path as that default).
        if tool_id == ToolId::FolderToPdf {
            return PathBuf::new();
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

        let parent = self.ensure_writable_dir(parent);

        if tool_id.is_multi_output() {
            return parent;
        }

        let suffix = output_suffix(tool_id);
        let ext = "pdf";
        parent.join(format!("{}{suffix}.{ext}", file_stem(&source)))
    }

    /// Cached [`crate::engine::pdf_io::dir_is_writable`] lookup.
    fn dir_writable(&mut self, dir: &std::path::Path) -> bool {
        if let Some(&ok) = self.writable_cache.get(dir) {
            return ok;
        }
        let ok = crate::engine::pdf_io::dir_is_writable(dir);
        self.writable_cache.insert(dir.to_path_buf(), ok);
        ok
    }

    /// Return `dir` if writable, otherwise a (cached, logged-once) fallback.
    fn ensure_writable_dir(&mut self, dir: PathBuf) -> PathBuf {
        if self.dir_writable(&dir) {
            return dir;
        }
        if let Some(cached) = self.fallback_cache.get(&dir) {
            return cached.clone();
        }
        let fallback = fallback_output_dir().unwrap_or_else(|| dir.clone());
        crate::engine::logging::log(
            "WARN",
            &format!(
                "输出目录不可写，自动改存：{} -> {}",
                dir.display(),
                fallback.display()
            ),
        );
        self.fallback_cache.insert(dir.clone(), fallback.clone());
        fallback
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
        let filtered: Vec<PathBuf> = if self.current == ToolId::FolderToPdf {
            // The batch tool takes folders, not files.
            dropped.into_iter().filter(|path| path.is_dir()).collect()
        } else {
            dropped
                .into_iter()
                .filter(|path| {
                    path.extension()
                        .map(|e| {
                            let e = e.to_string_lossy().to_ascii_lowercase();
                            accepted.iter().any(|a| *a == e)
                        })
                        .unwrap_or(false)
                })
                .collect()
        };

        if filtered.is_empty() {
            let detail = if self.current == ToolId::FolderToPdf {
                "拖入的内容不是文件夹（请拖入包含 JPG / PNG 图片的文件夹）".to_string()
            } else {
                format!("拖入的文件格式不被当前工具接受（{}）", accepted.join(" / "))
            };
            self.states.insert(
                self.current,
                TaskState::Failed {
                    error: ToolError::UnsupportedFormat {
                        path: None,
                        detail,
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

    fn show_panel(&mut self, ui: &mut egui::Ui) {
        let current = self.current;
        let form = &mut self.form;
        let preview = &mut self.preview;

        match current {
            ToolId::ImageToPdf => panels::image_to_pdf::show(ui, form, preview),
            ToolId::FolderToPdf => panels::folder_to_pdf::show(ui, form, preview),
            ToolId::Merge => panels::merge::show(ui, form),
            ToolId::Split => panels::split::show(ui, form, preview),
            ToolId::Rotate => panels::rotate::show(ui, form, preview),
            ToolId::DeletePages => panels::delete_pages::show(ui, form, preview),
            ToolId::Reorder => panels::reorder::show(ui, form, &mut self.reorder),
            ToolId::Extract => panels::extract::show(ui, form, preview),
            ToolId::TextWatermark => panels::text_watermark::show(ui, form),
            ToolId::ImageWatermark => panels::image_watermark::show(ui, form),
            ToolId::Encrypt => panels::encrypt::show(ui, form),
            ToolId::PdfToImage => panels::pdf_to_image::show(ui, form),
        }
    }
}

impl eframe::App for SparrowApp {
    /// egui 0.37 hands the app a `Ui` per frame instead of the `Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

        // The reorder panel's page list loads itself before anything renders,
        // so the action bar below never lags behind it.
        if self.current == ToolId::Reorder {
            panels::reorder::ensure_loaded(&mut self.form, &mut self.reorder);
        }

        self.refresh_readiness();

        let view = panel_view(self.current, &self.form, &self.reorder);
        let action = status::show(ui, self.state(), &view);

        let dark = matches!(ctx.theme(), egui::Theme::Dark);
        let side = sidebar::show(ui, self.current, self.icon.as_ref(), dark);
        if let Some(dark) = side.theme {
            self.set_theme_pref(&ctx, crate::ui::prefs::ThemePref::from_dark(dark));
        }
        if let Some(tool) = side.selected {
            if tool != self.current {
                self.current = tool;
                self.form.reset_inputs();
                self.reorder = ReorderState::default();
                self.states.insert(tool, TaskState::Idle);
            }
        }

        // CentralPanel last: it fills whatever the sidebar and bottom bar leave.
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.heading(self.current.label());
                ui.add_space(theme::GAP_S);
                ui.label(egui::RichText::new(self.current.description()).weak());
            });
            ui.add_space(theme::GAP_S);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.show_panel(ui);
                });
        });

        match action {
            PanelAction::Run => self.start(),
            PanelAction::Cancel => self.cancel(),
            PanelAction::RetryInDocuments => self.retry_in_documents(),
            PanelAction::None => {}
        }

        if self.poll_worker() || self.state().is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        // TEMP: capture the rendered frame and exit once animations settled.
        self.probe.frames += 1;
        if self.probe.out.is_some() {
            // egui stops repainting once idle; keep frames coming so the
            // screenshot lands after animations settle.
            ctx.request_repaint_after(std::time::Duration::from_millis(32));
            if self.probe.frames == 60 {
                let out = self.probe.out.clone().unwrap();
                ctx.request_screenshot(move |img| {
                    let w = img.size[0] as u32;
                    let h = img.size[1] as u32;
                    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(
                        &out,
                        &bytes,
                        w,
                        h,
                        image::ExtendedColorType::Rgba8,
                    );
                    std::process::exit(0);
                });
            }
        }
    }
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string())
}

/// Default file-name suffix per tool (single-output tools).
fn output_suffix(tool_id: ToolId) -> &'static str {
    match tool_id {
        ToolId::ImageToPdf | ToolId::Merge => "_merged",
        ToolId::Rotate => "_rotated",
        ToolId::DeletePages => "_deleted",
        ToolId::Reorder => "_reordered",
        ToolId::Extract => "_extracted",
        ToolId::TextWatermark | ToolId::ImageWatermark => "_watermarked",
        ToolId::Encrypt => "_secured",
        _ => "_out",
    }
}

/// First writable directory among common user folders, used when the source's
/// own folder is protected and the process lacks administrator rights.
///
/// Preferred target is a dedicated `Documents/SparrowPDF` folder: it is
/// always writable for the current user, predictable, and keeps redirected
/// outputs out of the user's normal document list.
fn fallback_output_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join("Documents").join("SparrowPDF"));
        candidates.push(home.join("Documents"));
        // Documents may be redirected into OneDrive (`%OneDrive%\Documents`).
        if let Some(onedrive) = std::env::var_os("OneDrive") {
            let base = PathBuf::from(onedrive).join("Documents");
            candidates.push(base.join("SparrowPDF"));
            candidates.push(base);
        }
        candidates.push(home.join("Desktop"));
        candidates.push(home.join("Downloads"));
        candidates.push(home);
    }
    candidates.push(std::env::temp_dir());
    candidates.into_iter().find(|dir| {
        // The preferred folder may not exist yet; creating it here means the
        // probe (and the later save) agree on the same location.
        let _ = std::fs::create_dir_all(dir);
        crate::engine::pdf_io::dir_is_writable(dir)
    })
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Fail fast when the resolved output location is not writable.
///
/// The folder batch tool with its default (empty) output path writes into
/// each folder itself; per-folder problems are tolerated and reported by the
/// tool, so this check skips that case.
fn preflight_output(
    tool_id: ToolId,
    request: &crate::tools::ToolRequest,
) -> Result<(), ToolError> {
    if request.output.as_os_str().is_empty() {
        return Ok(());
    }

    let dir = if tool_id.is_multi_output() {
        request.output.clone()
    } else {
        request
            .output
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    // A not-yet-existing output directory is fine as long as its nearest
    // existing ancestor is writable (`save_document` creates the rest).
    let mut probe = dir.clone();
    while !probe.as_os_str().is_empty() && !probe.is_dir() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }

    if crate::engine::pdf_io::dir_is_writable(&probe) {
        Ok(())
    } else {
        Err(ToolError::WriteFailed {
            path: dir,
            detail: "没有写入权限：目录可能受系统保护（如 Program Files），或需要管理员权限。请在「输出」中点击「选择输出目录…/选择输出文件…」改存到「文档」或桌面。".to_string(),
        })
    }
}

/// Decode the window icon into a texture for the sidebar header.
///
/// The bytes are the ones already embedded for the taskbar icon, so this adds
/// nothing to the binary.
fn load_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(ICON_PNG).ok()?;
    let rgba = img.to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        &rgba.into_raw(),
    );
    Some(ctx.load_texture("app_icon", image, egui::TextureOptions::default()))
}

/// Window-wide spacing and font sizes tuned for the 900x650 default size.
fn configure_style(ctx: &egui::Context) {
    use egui::{FontFamily::Proportional, FontId, TextStyle};

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size = egui::vec2(40.0, 24.0);

        // Snappier than the 0.2 s default; drives the sidebar crossfade.
        style.animation_time = 0.15;

        // One corner radius for every built-in control, so hand-styled chrome
        // and stock widgets agree.
        let radius = egui::CornerRadius::same(6);
        style.visuals.widgets.noninteractive.corner_radius = radius;
        style.visuals.widgets.inactive.corner_radius = radius;
        style.visuals.widgets.hovered.corner_radius = radius;
        style.visuals.widgets.active.corner_radius = radius;
        style.visuals.widgets.open.corner_radius = radius;

        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Small, FontId::new(11.5, Proportional)),
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
