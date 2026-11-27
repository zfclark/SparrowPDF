//! Shared domain types for SparrowPDF.
//!
//! Everything the UI layer is allowed to see lives here. No `lopdf`,
//! `medpdf`, `image` or `kamadak-exif` type ever crosses this boundary.

use std::fmt;
use std::path::PathBuf;

/// Identifier for every tool in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolId {
    ImageToPdf,
    FolderToPdf,
    Merge,
    Split,
    Rotate,
    DeletePages,
    Reorder,
    Extract,
    TextWatermark,
    ImageWatermark,
    ReplacePages,
    Encrypt,
    Compress,
    PdfToImage,
}

impl ToolId {
    /// Registry order, grouped the same way the sidebar renders them.
    ///
    /// Tools of one group are contiguous: the sidebar walks `ALL` and filters
    /// by group, so a stray member at the end would be listed under the first
    /// group but after every other group's rows.
    pub const ALL: [ToolId; 14] = [
        ToolId::ImageToPdf,
        ToolId::FolderToPdf,
        ToolId::PdfToImage,
        ToolId::Compress,
        ToolId::Merge,
        ToolId::Split,
        ToolId::Rotate,
        ToolId::DeletePages,
        ToolId::Reorder,
        ToolId::Extract,
        ToolId::TextWatermark,
        ToolId::ImageWatermark,
        ToolId::ReplacePages,
        ToolId::Encrypt,
    ];

    pub fn group(self) -> ToolGroup {
        match self {
            ToolId::ImageToPdf | ToolId::FolderToPdf | ToolId::PdfToImage | ToolId::Compress => {
                ToolGroup::Generate
            }
            ToolId::Merge
            | ToolId::Split
            | ToolId::Rotate
            | ToolId::DeletePages
            | ToolId::Reorder
            | ToolId::Extract => ToolGroup::Structure,
            ToolId::TextWatermark | ToolId::ImageWatermark | ToolId::ReplacePages => {
                ToolGroup::PageEdit
            }
            ToolId::Encrypt => ToolGroup::Security,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolId::ImageToPdf => "图片合并为 PDF",
            ToolId::FolderToPdf => "批量转换",
            ToolId::Merge => "PDF 合并",
            ToolId::Split => "PDF 拆分",
            ToolId::Rotate => "页面旋转",
            ToolId::DeletePages => "页面删除",
            ToolId::Reorder => "页面重排",
            ToolId::Extract => "页面提取",
            ToolId::TextWatermark => "文本水印",
            ToolId::ImageWatermark => "图片水印",
            ToolId::ReplacePages => "页面替换",
            ToolId::Encrypt => "加密与解密",
            ToolId::Compress => "PDF 压缩",
            ToolId::PdfToImage => "PDF 转图片",
        }
    }

    /// One-line summary shown under the panel heading.
    pub fn description(self) -> &'static str {
        match self {
            ToolId::ImageToPdf => "将多张 JPEG / PNG 合成为单个 PDF，并按 EXIF 方向校正",
            ToolId::FolderToPdf => "按模板将文件夹内的图片合成为 PDF，可按文件名分组把同名序号合并",
            ToolId::Merge => "按列表顺序合并多个 PDF",
            ToolId::Split => "按页码范围、每 N 页或逐页拆分",
            ToolId::Rotate => "按 90° 增量旋转指定页面，角度可累计",
            ToolId::DeletePages => "按页码范围删除页面",
            ToolId::Reorder => "拖动调整页序后重新生成文档",
            ToolId::Extract => "将指定页面另存为新的 PDF",
            ToolId::TextWatermark => "在页面上叠加文字水印，可设置字号、颜色、透明度与角度",
            ToolId::ImageWatermark => "将图片作为水印叠加到每一页",
            ToolId::ReplacePages => "用 PDF 页面或照片替换指定页面，可保持原页尺寸或跟随来源",
            ToolId::Encrypt => "AES-256 加密，或移除已有密码",
            ToolId::Compress => "按强度重新编码图片并清理未引用对象，保持页面顺序与文字不变",
            ToolId::PdfToImage => "逐页渲染为 PNG / JPEG，支持 36–600 DPI 与多文件批量导出",
        }
    }

    /// File extensions the tool accepts from the drag-and-drop surface.
    pub fn accepted_extensions(self) -> &'static [&'static str] {
        match self {
            ToolId::ImageToPdf | ToolId::FolderToPdf => &["jpg", "jpeg", "png"],
            ToolId::ImageWatermark => &["pdf", "jpg", "jpeg", "png"],
            // Slot 1 is the PDF being replaced, slot 2 the source page or photo.
            ToolId::ReplacePages => &["pdf", "jpg", "jpeg", "png"],
            ToolId::PdfToImage | ToolId::Compress => &["pdf"],
            _ => &["pdf"],
        }
    }

    /// How many file slots the tool takes. `None` means unbounded.
    pub fn max_inputs(self) -> Option<usize> {
        match self {
            ToolId::Merge | ToolId::ImageToPdf | ToolId::FolderToPdf | ToolId::PdfToImage => None,
            ToolId::ImageWatermark | ToolId::ReplacePages => Some(2),
            _ => Some(1),
        }
    }

    /// Whether this tool emits more than one file, so the UI asks for a folder.
    pub fn is_multi_output(self) -> bool {
        matches!(
            self,
            ToolId::Split | ToolId::PdfToImage | ToolId::FolderToPdf
        )
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGroup {
    /// Named 转换 rather than 生成: it holds the two image → PDF tools and
    /// PDF → image, which are all conversions.
    Generate,
    Structure,
    PageEdit,
    Security,
}

impl ToolGroup {
    pub const ALL: [ToolGroup; 4] = [
        ToolGroup::Generate,
        ToolGroup::Structure,
        ToolGroup::PageEdit,
        ToolGroup::Security,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolGroup::Generate => "转换",
            ToolGroup::Structure => "结构操作",
            ToolGroup::PageEdit => "页面编辑",
            ToolGroup::Security => "安全",
        }
    }

    pub fn tools(self) -> Vec<ToolId> {
        ToolId::ALL
            .iter()
            .copied()
            .filter(|t| t.group() == self)
            .collect()
    }
}

/// Task state machine: Idle -> Validating -> Ready -> Running -> Done/Failed/Cancelled.
#[derive(Debug, Clone, Default)]
pub enum TaskState {
    #[default]
    Idle,
    Validating,
    Ready,
    Running {
        progress: f32,
        message: String,
    },
    Done {
        summary: String,
        outputs: Vec<PathBuf>,
        /// Extra note shown under the summary (e.g. the output had to be
        /// redirected to a writable directory).
        note: Option<String>,
    },
    Failed {
        error: ToolError,
    },
    Cancelled,
}

impl TaskState {
    /// True while the tool must reject a second execute click.
    pub fn is_busy(&self) -> bool {
        matches!(self, TaskState::Validating | TaskState::Running { .. })
    }
}

/// Progress message sent from a worker thread back to the UI.
#[derive(Debug, Clone)]
pub enum WorkerMsg {
    Progress { fraction: f32, message: String },
    Finished(Result<ToolOutcome, ToolError>),
}

/// Final result of one tool execution.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub summary: String,
    pub outputs: Vec<PathBuf>,
}

impl ToolOutcome {
    pub fn success(summary: impl Into<String>, outputs: Vec<PathBuf>) -> Self {
        Self {
            summary: summary.into(),
            outputs,
        }
    }
}

/// Unified tool error. Covered exhaustively per the spec so the UI can map each
/// variant to a Chinese message without guessing.
#[derive(Debug, Clone)]
pub enum ToolError {
    /// Wrong extension or a file the tool cannot decode.
    UnsupportedFormat {
        path: Option<PathBuf>,
        detail: String,
    },
    ReadFailed {
        path: PathBuf,
        detail: String,
    },
    WriteFailed {
        path: PathBuf,
        detail: String,
    },
    InvalidPageRange {
        detail: String,
    },
    PasswordRequired {
        path: PathBuf,
    },
    WrongPassword {
        path: PathBuf,
    },
    /// Output verification failed; the produced file has already been deleted.
    ValidationFailed {
        detail: String,
    },
    /// The planned output already exists and the request asked to be told
    /// instead of overwriting. Nothing has been written when this is returned.
    OutputExists {
        paths: Vec<PathBuf>,
    },
    /// `ConflictPolicy::Unique` ran out of serial numbers (`-2` … `-9999`).
    /// Nothing has been written when this is returned.
    OutputNameExhausted {
        path: PathBuf,
    },
    Cancelled,
    /// Some inputs succeeded, some did not.
    ///
    /// `outputs` carries the artifacts that *were* produced: a partial run must
    /// stay openable from the result card instead of looking like a total
    /// failure.
    PartialSuccess {
        succeeded: usize,
        failed: usize,
        detail: String,
        outputs: Vec<PathBuf>,
    },
    Internal {
        detail: String,
    },
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::UnsupportedFormat { path, detail } => match path {
                Some(p) => write!(f, "不支持的格式：{}（{detail}）", p.display()),
                None => write!(f, "不支持的格式：{detail}"),
            },
            ToolError::ReadFailed { path, detail } => {
                write!(f, "文件读取失败：{}（{detail}）", path.display())
            }
            ToolError::WriteFailed { path, detail } => {
                write!(f, "文件写入失败：{}（{detail}）", path.display())
            }
            ToolError::InvalidPageRange { detail } => write!(f, "无效页码：{detail}"),
            ToolError::PasswordRequired { path } => {
                write!(f, "需要密码：{}", path.display())
            }
            ToolError::WrongPassword { path } => {
                write!(f, "密码错误：{}", path.display())
            }
            ToolError::ValidationFailed { detail } => write!(f, "产物校验失败：{detail}"),
            ToolError::OutputExists { paths } => {
                let first = paths
                    .first()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                write!(f, "目标文件已存在：{first}（共 {} 个）", paths.len())
            }
            ToolError::OutputNameExhausted { path } => write!(
                f,
                "同名文件太多，无法自动编号：{}（请手动改名或另选目录）",
                path.display()
            ),
            ToolError::Cancelled => f.write_str("已取消"),
            ToolError::PartialSuccess {
                succeeded,
                failed,
                detail,
                ..
            } => write!(
                f,
                "部分成功：成功 {succeeded} 个，失败 {failed} 个（{detail}）"
            ),
            ToolError::Internal { detail } => write!(f, "内部错误：{detail}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(err: std::io::Error) -> Self {
        ToolError::Internal {
            detail: err.to_string(),
        }
    }
}

/// Output options shared by several tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRangeMode {
    /// `1-3,7`
    Explicit,
    /// every N pages
    EveryN,
    /// one output file per page
    SinglePages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationDegrees {
    Deg90,
    Deg180,
    Deg270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkPlacement {
    Center,
    Tiled,
}

/// What to do when a planned output path is already taken.
///
/// The default is [`ConflictPolicy::Report`]: a tool computes every path it
/// intends to write, reports the clash, and only writes once the user picked a
/// policy. Overwriting therefore never happens silently, and `Unique` keeps the
/// previous result instead of destroying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Return [`ToolError::OutputExists`] carrying every clashing path.
    #[default]
    Report,
    /// Replace the existing file(s).
    Overwrite,
    /// Keep the existing file(s) and suffix the new names with `-2`, `-3`, …
    Unique,
}

/// A single file the user has staged for the active tool.
///
/// `Hash` + `Debug` are needed by `egui_dnd`'s blanket `DragDropItem` impl.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputFile {
    pub path: PathBuf,
}

impl InputFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}
