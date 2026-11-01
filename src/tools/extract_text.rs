//! Tool 12: extract text to a plain text file.
//!
//! Uses `lopdf`'s object-level text extraction (`Document::extract_text`), which
//! walks each page's content stream and decodes the font encodings.

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolRequest, check_cancel,
    split::{ensure_pdf, single_input},
};

pub struct ExtractText;

impl Tool for ExtractText {
    fn id(&self) -> ToolId {
        ToolId::ExtractText
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let path = single_input(req)?;

        progress.report(0.1, "读取 PDF".to_string());
        check_cancel(cancel)?;

        let doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&doc);
        if total == 0 {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "文档没有页面".to_string(),
            });
        }

        let page_numbers: Vec<u32> = (1..=total).collect();
        let mut text = String::new();

        // Extract page by page so progress stays meaningful and cancellation is
        // honoured on long documents.
        for (index, page) in page_numbers.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                0.1 + 0.8 * (index as f32 / total as f32),
                format!("提取第 {page}/{total} 页文本"),
            );
            match doc.extract_text(&[*page]) {
                Ok(chunk) => {
                    text.push_str(&chunk);
                    if !chunk.ends_with('\n') {
                        text.push('\n');
                    }
                }
                Err(err) => {
                    // A page with a broken font must not lose the whole document.
                    text.push_str(&format!("[第 {page} 页文本提取失败：{err}]\n"));
                }
            }
        }

        if text.trim().is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "该 PDF 不含可提取的文本（可能是扫描件）".to_string(),
            });
        }

        progress.report(0.95, "写入文本文件".to_string());
        check_cancel(cancel)?;
        pdf_io::write_file(&req.output, text.as_bytes())?;
        validate::assert_non_empty_text(&req.output)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已提取 {total} 页文本，共 {} 字", text.chars().count()),
            vec![req.output.clone()],
        ))
    }
}
