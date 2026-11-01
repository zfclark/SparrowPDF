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
    Merge,
    Split,
    Rotate,
    DeletePages,
    Reorder,
    Extract,
    TextWatermark,
    ImageWatermark,
    Encrypt,
    PdfToImage,
    ExtractText,
}

impl ToolId {
    /// Registry order, grouped the same way the sidebar renders them.
    pub const ALL: [ToolId; 12] = [
        ToolId::ImageToPdf,
        ToolId::Merge,
        ToolId::Split,
        ToolId::Rotate,
        ToolId::DeletePages,
        ToolId::Reorder,
        ToolId::Extract,
        ToolId::TextWatermark,
        ToolId::ImageWatermark,
        ToolId::Encrypt,
        ToolId::PdfToImage,
        ToolId::ExtractText,
    ];

    pub fn group(self) -> ToolGroup {
        match self {
            ToolId::ImageToPdf => ToolGroup::Generate,
            ToolId::Merge
            | ToolId::Split
            | ToolId::Rotate
            | ToolId::DeletePages
            | ToolId::Reorder
            | ToolId::Extract => ToolGroup::Structure,
            ToolId::TextWatermark | ToolId::ImageWatermark => ToolGroup::PageEdit,
            ToolId::Encrypt => ToolGroup::Security,
            ToolId::PdfToImage | ToolId::ExtractText => ToolGroup::Convert,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolId::ImageToPdf => "图片合并为 PDF",
            ToolId::Merge => "PDF 合并",
            ToolId::Split => "PDF 拆分",
            ToolId::Rotate => "页面旋转",
            ToolId::DeletePages => "页面删除",
            ToolId::Reorder => "页面重排",
            ToolId::Extract => "页面提取",
            ToolId::TextWatermark => "文本水印",
            ToolId::ImageWatermark => "图片水印",
            ToolId::Encrypt => "加密与解密",
            ToolId::PdfToImage => "PDF 转图片",
            ToolId::ExtractText => "文本提取",
        }
    }

    /// File extensions the tool accepts from the drag-and-drop surface.
    pub fn accepted_extensions(self) -> &'static [&'static str] {
        match self {
            ToolId::ImageToPdf => &["jpg", "jpeg", "png"],
            ToolId::ImageWatermark => &["pdf", "jpg", "jpeg", "png"],
            ToolId::PdfToImage => &["pdf"],
            _ => &["pdf"],
        }
    }

    /// How many file slots the tool takes. `None` means unbounded.
    pub fn max_inputs(self) -> Option<usize> {
        match self {
            ToolId::Merge | ToolId::ImageToPdf => None,
            ToolId::ImageWatermark => Some(2),
            _ => Some(1),
        }
    }

    /// Whether this tool emits more than one file, so the UI asks for a folder.
    pub fn is_multi_output(self) -> bool {
        matches!(
            self,
            ToolId::Split | ToolId::PdfToImage
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
    Generate,
    Structure,
    PageEdit,
    Security,
    Convert,
}

impl ToolGroup {
    pub const ALL: [ToolGroup; 5] = [
        ToolGroup::Generate,
        ToolGroup::Structure,
        ToolGroup::PageEdit,
        ToolGroup::Security,
        ToolGroup::Convert,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolGroup::Generate => "生成",
            ToolGroup::Structure => "结构操作",
            ToolGroup::PageEdit => "页面编辑",
            ToolGroup::Security => "安全",
            ToolGroup::Convert => "转换",
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
    },
    Failed {
        error: ToolError,
    },
    Cancelled,
}

impl TaskState {
    pub fn label(&self) -> String {
        match self {
            TaskState::Idle => "空闲".to_string(),
            TaskState::Validating => "校验中".to_string(),
            TaskState::Ready => "就绪".to_string(),
            TaskState::Running { progress, .. } => {
                format!("处理中 {}%", (progress * 100.0).round() as i32)
            }
            TaskState::Done { .. } => "完成".to_string(),
            TaskState::Failed { .. } => "失败".to_string(),
            TaskState::Cancelled => "已取消".to_string(),
        }
    }

    pub fn progress(&self) -> Option<f32> {
        match self {
            TaskState::Running { progress, .. } => Some(*progress),
            TaskState::Done { .. } => Some(1.0),
            _ => None,
        }
    }

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
    UnsupportedFormat { path: Option<PathBuf>, detail: String },
    ReadFailed { path: PathBuf, detail: String },
    WriteFailed { path: PathBuf, detail: String },
    InvalidPageRange { detail: String },
    PasswordRequired { path: PathBuf },
    WrongPassword { path: PathBuf },
    /// Output verification failed; the produced file has already been deleted.
    ValidationFailed { detail: String },
    Cancelled,
    /// Some inputs succeeded, some did not.
    PartialSuccess { succeeded: usize, failed: usize, detail: String },
    Internal { detail: String },
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
            ToolError::Cancelled => f.write_str("已取消"),
            ToolError::PartialSuccess {
                succeeded,
                failed,
                detail,
            } => write!(f, "部分成功：成功 {succeeded} 个，失败 {failed} 个（{detail}）"),
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
