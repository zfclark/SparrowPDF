//! Tool registry.
//!
//! Every tool implements [`Tool`]. The UI only ever talks to this module plus
//! `model`; it never touches `lopdf`, `medpdf`, `medpdf_image`, `image` or
//! `kamadak_exif` directly.

pub mod delete_pages;
pub mod encrypt;
pub mod extract;
pub mod folder_to_pdf;
pub mod image_embed;
pub mod image_to_pdf;
pub mod image_watermark;
pub mod merge;
pub mod pdf_to_image;
pub mod reorder;
pub mod rotate;
pub mod split;
pub mod text_watermark;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::model::{ImageFormat, InputFile, PageRangeMode, RotationDegrees, ToolError, ToolId, ToolOutcome, WatermarkPlacement, WorkerMsg};

/// Cancellation flag shared between the UI thread and a worker.
pub type CancelFlag = Arc<AtomicBool>;

/// Per-tool options. One variant per tool keeps [`Tool::execute`] a single
/// uniform signature while staying type-safe.
#[derive(Debug, Clone)]
pub enum ToolOptions {
    ImageToPdf {
        /// Force a page orientation instead of deriving it per image.
        force_orientation: Option<Orientation>,
        /// Rotate an image 90° when its orientation conflicts with the forced
        /// page orientation (landscape image on a portrait page and vice
        /// versa), so it fills the page instead of shrinking to a strip.
        auto_rotate: bool,
    },
    /// Batch conversion: one PDF per input folder.
    FolderToPdf {
        /// How images are laid out on pages.
        mode: BatchPageMode,
        /// Page orientation for one-image-per-page mode.
        force_orientation: Option<Orientation>,
        /// Page orientation for two-per-page mode: landscape pages hold the
        /// two slots side by side, portrait pages stack them vertically.
        pair_orientation: Orientation,
        /// Whether the two-per-page merge follows the user's explicit
        /// `merge_spec` instead of pairing every image sequentially.
        use_merge_spec: bool,
        /// Merge spec, e.g. `1-2,5-6` (1-based image numbers). Only used
        /// when [`Self::use_merge_spec`] is set; only the listed images are
        /// merged, everything else stays one-per-page.
        merge_spec: String,
        /// Rotate an image 90° when its orientation conflicts with its slot.
        auto_rotate: bool,
    },
    Merge,
    Split {
        mode: PageRangeMode,
        /// `1-3,7` for [`PageRangeMode::Explicit`].
        spec: String,
        /// N for [`PageRangeMode::EveryN`].
        every_n: usize,
    },
    Rotate {
        pages: String,
        degrees: RotationDegrees,
    },
    DeletePages {
        pages: String,
    },
    Reorder {
        /// 0-based output order expressed as source page indices.
        order: Vec<usize>,
    },
    Extract {
        pages: String,
    },
    TextWatermark {
        text: String,
        font_size: f32,
        color: [u8; 3],
        opacity: f32,
        rotation: f32,
        placement: WatermarkPlacement,
    },
    ImageWatermark {
        scale: f32,
        opacity: f32,
        position: WatermarkPlacement,
    },
    Encrypt {
        /// `true` encrypts, `false` removes existing encryption.
        encrypt: bool,
        user_password: String,
        owner_password: String,
        allow_print: bool,
        allow_copy: bool,
        allow_modify: bool,
    },
    PdfToImage {
        dpi: u32,
        format: ImageFormat,
    },
}

impl ToolOptions {
    pub fn default_for(id: ToolId) -> Self {
        match id {
            ToolId::ImageToPdf => ToolOptions::ImageToPdf {
                force_orientation: None,
                auto_rotate: true,
            },
            ToolId::FolderToPdf => ToolOptions::FolderToPdf {
                mode: BatchPageMode::Single,
                force_orientation: None,
                pair_orientation: Orientation::Landscape,
                use_merge_spec: false,
                merge_spec: String::new(),
                auto_rotate: true,
            },
            ToolId::Merge => ToolOptions::Merge,
            ToolId::Split => ToolOptions::Split {
                mode: PageRangeMode::Explicit,
                spec: String::new(),
                every_n: 1,
            },
            ToolId::Rotate => ToolOptions::Rotate {
                pages: "all".to_string(),
                degrees: RotationDegrees::Deg90,
            },
            ToolId::DeletePages => ToolOptions::DeletePages {
                pages: String::new(),
            },
            ToolId::Reorder => ToolOptions::Reorder { order: Vec::new() },
            ToolId::Extract => ToolOptions::Extract {
                pages: String::new(),
            },
            ToolId::TextWatermark => ToolOptions::TextWatermark {
                text: String::new(),
                font_size: 48.0,
                color: [128, 128, 128],
                opacity: 0.3,
                rotation: 45.0,
                placement: WatermarkPlacement::Center,
            },
            ToolId::ImageWatermark => ToolOptions::ImageWatermark {
                scale: 0.3,
                opacity: 0.4,
                position: WatermarkPlacement::Center,
            },
            ToolId::Encrypt => ToolOptions::Encrypt {
                encrypt: true,
                user_password: String::new(),
                owner_password: String::new(),
                allow_print: true,
                allow_copy: true,
                allow_modify: true,
            },
            ToolId::PdfToImage => ToolOptions::PdfToImage {
                dpi: 300,
                format: ImageFormat::Png,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Page layout for the folder batch tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPageMode {
    /// Each image becomes its own page.
    Single,
    /// Two images share one landscape page.
    TwoPerPage,
}

/// Everything one execution needs. `output` is a file for single-output tools
/// and a directory for tools that emit several files.
#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub inputs: Vec<InputFile>,
    pub output: PathBuf,
    pub options: ToolOptions,
    /// Password supplied by the user for encrypted inputs, if any.
    pub password: Option<String>,
}

/// Progress sink handed to [`Tool::execute`]. Cheap to clone, ignores send
/// errors so a dropped UI receiver never fails the work.
#[derive(Clone)]
pub struct Progress {
    tx: Sender<WorkerMsg>,
}

impl Progress {
    pub fn new(tx: Sender<WorkerMsg>) -> Self {
        Self { tx }
    }

    pub fn report(&self, fraction: f32, message: impl Into<String>) {
        let _ = self.tx.send(WorkerMsg::Progress {
            fraction: fraction.clamp(0.0, 1.0),
            message: message.into(),
        });
    }
}

/// Return `Err(ToolError::Cancelled)` when the user asked to stop.
pub fn check_cancel(cancel: &CancelFlag) -> Result<(), ToolError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

/// The one interface every tool implements.
pub trait Tool: Send + Sync {
    fn id(&self) -> ToolId;

    /// Display name for this tool.
    ///
    /// The UI renders headings from [`ToolId::label`] so it can reuse a
    /// `&'static str` lookup, but the interface keeps `name` because the spec
    /// fixes the four-method shape (id / name / validate / execute).
    #[allow(dead_code, reason = "fixed Tool interface member; UI renders via ToolId::label")]
    fn name(&self) -> &'static str {
        self.id().label()
    }

    /// Cheap, side-effect free pre-flight check. Must not touch the filesystem
    /// beyond `metadata`/existence checks.
    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError>;

    /// Do the work. Must re-open its own outputs and assert invariants before
    /// returning `Ok`, deleting the artifact when verification fails.
    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError>;
}

/// Build the tool that handles `id`.
pub fn tool_for(id: ToolId) -> Box<dyn Tool> {
    match id {
        ToolId::ImageToPdf => Box::new(image_to_pdf::ImageToPdf),
        ToolId::FolderToPdf => Box::new(folder_to_pdf::FolderToPdf),
        ToolId::Merge => Box::new(merge::Merge),
        ToolId::Split => Box::new(split::Split),
        ToolId::Rotate => Box::new(rotate::Rotate),
        ToolId::DeletePages => Box::new(delete_pages::DeletePages),
        ToolId::Reorder => Box::new(reorder::Reorder),
        ToolId::Extract => Box::new(extract::Extract),
        ToolId::TextWatermark => Box::new(text_watermark::TextWatermark),
        ToolId::ImageWatermark => Box::new(image_watermark::ImageWatermark),
        ToolId::Encrypt => Box::new(encrypt::Encrypt),
        ToolId::PdfToImage => Box::new(pdf_to_image::PdfToImage),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers used by more than one tool.
// ---------------------------------------------------------------------------

/// Ensure a directory exists, creating parents as needed.
pub fn ensure_dir(dir: &std::path::Path) -> Result<(), ToolError> {
    std::fs::create_dir_all(dir).map_err(|e| ToolError::WriteFailed {
        path: dir.to_path_buf(),
        detail: crate::engine::pdf_io::write_error_detail(&e),
    })
}
