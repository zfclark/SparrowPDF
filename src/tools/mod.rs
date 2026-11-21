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

use crate::model::{
    ImageFormat, InputFile, PageRangeMode, RotationDegrees, ToolError, ToolId, ToolOutcome,
    WatermarkPlacement, WorkerMsg,
};

pub use crate::model::ConflictPolicy;

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
        /// Layout preset the current settings came from. Kept for the panel's
        /// description and for the tool's lenient handling of the preset's own
        /// merge spec; every effective value lives in the fields below, so a
        /// preset is only ever applied by copying its values in.
        template: &'static BatchTemplate,
        /// Whether the folder becomes one PDF or one PDF per base name.
        grouping: BatchGrouping,
        /// How images are laid out on pages.
        mode: BatchPageMode,
        /// Page orientation for one-image-per-page mode.
        force_orientation: Option<Orientation>,
        /// Page orientation for two-per-page mode: landscape pages hold the
        /// two slots side by side, portrait pages stack them vertically.
        pair_orientation: Orientation,
        /// Paper size of every page in the document.
        page_size: PageSize,
        /// Fixed slot size for card-size copies; `None` halves the page.
        slot_size: Option<(f32, f32)>,
        /// Whether the two-per-page merge follows the user's explicit
        /// `merge_spec` instead of pairing every image sequentially.
        use_merge_spec: bool,
        /// Merge spec, e.g. `1-2,5-6` (1-based image numbers). Only used
        /// when [`Self::use_merge_spec`] is set; only the listed images are
        /// merged, everything else stays one-per-page.
        merge_spec: String,
        /// Rotate an image 90° when its orientation conflicts with its slot.
        auto_rotate: bool,
        /// Keep the slot margin on single-image pages of a two-per-page
        /// layout. Off means those pages are converted full-bleed, which is
        /// what a form preset (入团志愿书) needs for "other pages as usual".
        inset_single_pages: bool,
        /// How hard to shrink oversized scans. Independent of the layout
        /// preset: switching templates must not silently change print quality.
        scan_quality: ScanQuality,
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
            ToolId::FolderToPdf => {
                // The default layout is the table's first entry: presets and
                // defaults can never drift apart.
                let template = default_batch_template();
                let mut options = ToolOptions::FolderToPdf {
                    template,
                    grouping: BatchGrouping::ByFolder,
                    mode: BatchPageMode::Single,
                    force_orientation: None,
                    pair_orientation: Orientation::Landscape,
                    page_size: PageSize::A4,
                    slot_size: None,
                    use_merge_spec: false,
                    merge_spec: String::new(),
                    auto_rotate: true,
                    inset_single_pages: true,
                    scan_quality: ScanQuality::Balanced,
                };
                options.apply_batch_template(template);
                options
            }
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

/// How one input folder is split into output documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchGrouping {
    /// One PDF per folder — the original behaviour.
    #[default]
    ByFolder,
    /// One PDF per shared base name: `xxx_1.png` and `xxx_2.png` become
    /// `xxx.pdf`, `yyy_1.png` becomes `yyy.pdf`.
    ByName,
}


/// How much of a scan's resolution the batch tool is allowed to throw away.
///
/// A phone photo or a 600 DPI flatbed scan carries far more pixels than a
/// printed sheet can resolve, and the surplus is pure file size: 13 A4 pages
/// of 600 DPI PNGs easily make a 60 MB PDF. Each level caps the *effective*
/// resolution once the image is drawn on its page, so the pixels that do get
/// written are exactly the ones the printer would use.
///
/// The levels are also the starting points of the size guard in
/// `folder_to_pdf`: an output above [`SCAN_SIZE_LIMIT`] is re-rendered one step
/// tighter until it fits (see `image_embed::ScanBudget::tightened`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanQuality {
    /// 150 DPI — smallest. Fine for reading on screen and for filing.
    Small,
    /// 300 DPI — the default: the common print standard for a scanned form,
    /// and what a laser printer can actually resolve on a sheet.
    #[default]
    Balanced,
    /// Keep the source pixels untouched (no re-encode, no size guard).
    Original,
}

/// Output size above which a batch PDF is re-rendered with a tighter scan
/// budget: 20 MiB.
pub const SCAN_SIZE_LIMIT: u64 = 20 * 1024 * 1024;

impl ScanQuality {
    pub const ALL: [ScanQuality; 3] = [
        ScanQuality::Balanced,
        ScanQuality::Small,
        ScanQuality::Original,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScanQuality::Small => "更小体积（150 DPI）",
            ScanQuality::Balanced => "标准（300 DPI，推荐）",
            ScanQuality::Original => "不压缩（原图直通）",
        }
    }

    /// Resolution / quality budget this level starts from; `None` disables
    /// both the re-encode and the size guard.
    pub(crate) fn budget(self) -> Option<crate::tools::image_embed::ScanBudget> {
        match self {
            ScanQuality::Small => Some(crate::tools::image_embed::ScanBudget::new(150.0, 78)),
            ScanQuality::Balanced => Some(crate::tools::image_embed::ScanBudget::new(300.0, 85)),
            ScanQuality::Original => None,
        }
    }
}

// ------------------------------------------------------------------ presets
//
// Presets are pure data. Adding a template means appending one entry to
// [`BATCH_TEMPLATES`] — no match arm, no new function, no UI change: the panel
// renders whatever the table contains and the tool reads whatever the applied
// values are.

/// Paper size of the pages produced by the image → PDF tools.
///
/// Dimensions are PDF points (1 pt = 1/72 inch), portrait orientation. 标准
/// exposes all four; the named templates pick one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    A4,
    A5,
    B5,
    Letter,
}

impl PageSize {
    pub const ALL: [PageSize; 4] = [PageSize::A4, PageSize::A5, PageSize::B5, PageSize::Letter];

    pub fn label(self) -> &'static str {
        match self {
            PageSize::A4 => "A4（210 × 297 mm）",
            PageSize::A5 => "A5（148 × 210 mm）",
            PageSize::B5 => "B5（176 × 250 mm）",
            PageSize::Letter => "Letter（8.5 × 11 in）",
        }
    }

    /// Portrait width / height, in points.
    pub fn dimensions(self) -> (f32, f32) {
        match self {
            PageSize::A4 => (595.28, 841.89),
            PageSize::A5 => (419.53, 595.28),
            PageSize::B5 => (498.90, 708.66),
            PageSize::Letter => (612.0, 792.0),
        }
    }

    /// Width / height of one page of this size in `orientation`.
    pub fn page(self, orientation: Orientation) -> (f32, f32) {
        let (w, h) = self.dimensions();
        match orientation {
            Orientation::Portrait => (w, h),
            Orientation::Landscape => (h, w),
        }
    }
}

/// Every layout value one preset owns.
///
/// [`ToolOptions::apply_batch_template`] copies these fields verbatim, so this
/// struct is the complete contract between a template and the tool — a new
/// preset cannot forget to set a knob that the layout reads. Named templates
/// fix all of them; only 标准 (and the card direction) expose any of them in
/// the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchPreset {
    /// Paper size of every page.
    pub page_size: PageSize,
    /// Page orientation of single-image pages; `None` follows each image.
    pub orientation: Option<Orientation>,
    /// Direction of a two-per-page layout, and of its single-image pages.
    pub pair_orientation: Orientation,
    /// Two images share one page.
    pub two_per_page: bool,
    /// Merge only the listed images (1-based, e.g. `2-3`); `None` pairs every
    /// image sequentially when `two_per_page` is set.
    pub merge_spec: Option<&'static str>,
    /// Fixed slot size in points (card-size copies such as 身份证). `None`
    /// gives each image half of the page.
    pub slot_size: Option<(f32, f32)>,
    /// Rotate an image 90° when its orientation conflicts with its page/slot.
    pub auto_rotate: bool,
    /// Keep the slot margin on single-image pages of a two-per-page layout.
    pub inset_single_pages: bool,
}

/// Which parameters the user may still change for a template.
///
/// A named template fixes the sheet and the arrangement, because a changed
/// paper size or direction is what makes a copy come out wrong. 标准 is the
/// free-form preset and exposes everything; a card copy only leaves direction
/// and rotation open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchAdjust {
    pub page_size: bool,
    pub page_layout: bool,
    /// Page direction (also flips a two-per-page layout).
    pub page_orientation: bool,
    /// Rotate images whose orientation does not match their slot.
    pub auto_rotate: bool,
    /// Expose the merge switch (标准's pairing, 入团志愿书's fixed pair).
    pub merge: bool,
}

impl BatchAdjust {
    /// Direction and rotation stay with the user (card copies, A5 booklet).
    pub const ORIENTATION: Self = Self {
        page_size: false,
        page_layout: false,
        page_orientation: true,
        auto_rotate: true,
        merge: false,
    };
    /// 入团志愿书: the merge switch is the only thing to change.
    pub const MERGE: Self = Self {
        page_size: false,
        page_layout: false,
        page_orientation: false,
        auto_rotate: false,
        merge: true,
    };
    /// 标准: no preset at all, every knob is the user's.
    pub const FULL: Self = Self {
        page_size: true,
        page_layout: true,
        page_orientation: true,
        auto_rotate: true,
        merge: true,
    };
}

/// One batch template: what it is called, what the output must look like, and
/// the values it sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchTemplate {
    /// Stable identifier, independent of the display label.
    pub key: &'static str,
    pub label: &'static str,
    /// Format requirement shown under the dropdown: paper, images per page,
    /// arrangement. Two sentences at most.
    pub format: &'static str,
    /// Which knobs stay adjustable in the panel.
    pub adjust: BatchAdjust,
    pub preset: BatchPreset,
}

impl BatchTemplate {
    /// Whether the preset's own merge spec is the one currently in use.
    ///
    /// A template's spec is advisory: a folder with fewer images keeps those
    /// pages separate instead of failing the whole batch. A spec the user
    /// typed is strict — a typo must not silently change the layout.
    pub fn owns_merge_spec(&self, spec: &str) -> bool {
        self.preset.merge_spec == Some(spec.trim())
    }
}

/// 二代身份证尺寸 85.6 × 54 mm，换算成点。
const ID_CARD_W: f32 = 242.65;
const ID_CARD_H: f32 = 153.07;

/// The single source of templates.
///
/// Append a row to add one: the panel lists it, applies it and shows its
/// format text without further code. Keep `key` stable — it identifies the
/// preset across builds if templates are ever persisted to the preferences
/// file.
pub static BATCH_TEMPLATES: &[BatchTemplate] = &[
    BatchTemplate {
        key: "standard",
        label: "标准",
        format: "不套用预设：A4、每页一张，页面方向随图片。纸张、方向、每页张数与合并均在下方调整。",
        adjust: BatchAdjust::FULL,
        preset: BatchPreset {
            page_size: PageSize::A4,
            orientation: None,
            pair_orientation: Orientation::Landscape,
            two_per_page: false,
            merge_spec: None,
            slot_size: None,
            auto_rotate: true,
            inset_single_pages: true,
        },
    },
    BatchTemplate {
        key: "id-card",
        label: "身份证",
        format: "A4 每页两张，单张按 85.6 × 54 mm 原尺寸居中，不缩放；竖版上下排列，横版左右排列。扫描件按正面、反面命名为 1、2。",
        adjust: BatchAdjust::ORIENTATION,
        preset: BatchPreset {
            page_size: PageSize::A4,
            orientation: Some(Orientation::Portrait),
            pair_orientation: Orientation::Portrait,
            two_per_page: true,
            merge_spec: None,
            slot_size: Some((ID_CARD_W, ID_CARD_H)),
            // A card scan is already the right way up; turning it would put
            // the portrait photo sideways.
            auto_rotate: false,
            inset_single_pages: true,
        },
    },
    BatchTemplate {
        key: "youth-league-form",
        label: "入团志愿书",
        format: "A4 竖版。第 2、3 页合印一页，各旋转 90°、上下各占半页；其余页面单张成页。图片按 1–5 命名，不足 3 张时不合并。",
        adjust: BatchAdjust::MERGE,
        preset: BatchPreset {
            page_size: PageSize::A4,
            orientation: Some(Orientation::Portrait),
            pair_orientation: Orientation::Portrait,
            two_per_page: true,
            merge_spec: Some("2-3"),
            slot_size: None,
            auto_rotate: true,
            inset_single_pages: false,
        },
    },
    BatchTemplate {
        key: "a5-booklet",
        label: "A5 小册子",
        format: "A5 每页一张。A4 纸对折后两页并排，适合装订成册；页面方向可在下方调整。",
        adjust: BatchAdjust::ORIENTATION,
        preset: BatchPreset {
            page_size: PageSize::A5,
            orientation: Some(Orientation::Portrait),
            pair_orientation: Orientation::Landscape,
            two_per_page: false,
            merge_spec: None,
            slot_size: None,
            auto_rotate: true,
            inset_single_pages: true,
        },
    },
];


/// The preset used for a fresh session (the table's first row).
pub fn default_batch_template() -> &'static BatchTemplate {
    &BATCH_TEMPLATES[0]
}

/// Look a preset up by its stable key.
///
/// The panel talks to templates through the table, so this is only exercised
/// by tests today; it is the entry point for persisting the choice later.
#[allow(
    dead_code,
    reason = "key lookup for preset persistence; exercised by integration tests"
)]
pub fn batch_template_by_key(key: &str) -> Option<&'static BatchTemplate> {
    BATCH_TEMPLATES.iter().find(|template| template.key == key)
}

impl ToolOptions {
    /// Copy `template`'s preset into a folder-batch option set.
    ///
    /// Every value a preset owns is overwritten so the panel and the tool can
    /// never disagree; options of other tools are untouched.
    pub fn apply_batch_template(&mut self, template: &'static BatchTemplate) {
        let ToolOptions::FolderToPdf {
            template: current,
            // Grouping is a run mode, not a layout: switching template must not
            // change how many PDFs a folder produces.
            grouping: _,
            mode,
            force_orientation,
            pair_orientation,
            page_size,
            slot_size,
            use_merge_spec,
            merge_spec,
            auto_rotate,
            inset_single_pages,
            // Print quality is a user preference, not part of a layout: a
            // template switch (入团志愿书 → 身份证) must not reset it.
            scan_quality: _,
        } = self
        else {
            return;
        };
        let preset = &template.preset;
        *current = template;
        *mode = if preset.two_per_page {
            BatchPageMode::TwoPerPage
        } else {
            BatchPageMode::Single
        };
        *force_orientation = preset.orientation;
        *pair_orientation = preset.pair_orientation;
        *page_size = preset.page_size;
        *slot_size = preset.slot_size;
        *use_merge_spec = preset.merge_spec.is_some();
        *merge_spec = preset.merge_spec.unwrap_or_default().to_string();
        *auto_rotate = preset.auto_rotate;
        *inset_single_pages = preset.inset_single_pages;
    }
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
    /// How to handle outputs that already exist on disk.
    pub conflict: ConflictPolicy,
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
    #[allow(
        dead_code,
        reason = "fixed Tool interface member; UI renders via ToolId::label"
    )]
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
