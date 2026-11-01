//! Tool 5: delete pages by page number.

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest, check_cancel,
    split::{ensure_pdf, single_input},
};

pub struct DeletePages;

impl Tool for DeletePages {
    fn id(&self) -> ToolId {
        ToolId::DeletePages
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::DeletePages { pages } = &req.options {
            if pages.trim().is_empty() {
                return Err(ToolError::InvalidPageRange {
                    detail: "请填写要删除的页码".to_string(),
                });
            }
        }
        Ok(())
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let path = single_input(req)?;
        let spec = match &req.options {
            ToolOptions::DeletePages { pages } => pages.clone(),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let mut doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&doc);
        let targets = pdf_io::parse_pages(&spec, total)?;

        if targets.len() >= total as usize {
            return Err(ToolError::InvalidPageRange {
                detail: "不能删除全部页面".to_string(),
            });
        }

        // Delete from the highest page number down, so the lower numbers still
        // refer to the pages the user asked to remove.
        let mut descending = targets.clone();
        descending.sort_unstable_by(|a, b| b.cmp(a));

        for (index, page_num) in descending.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / descending.len() as f32,
                format!("删除第 {page_num} 页"),
            );
            medpdf::delete_page(&mut doc, *page_num).map_err(|e| ToolError::Internal {
                detail: format!("删除第 {page_num} 页失败：{e}"),
            })?;
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &req.output)?;

        let expected = total - targets.len() as u32;
        validate::assert_page_count(&req.output, expected, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已删除 {} 页，剩余 {expected} 页", targets.len()),
            vec![req.output.clone()],
        ))
    }
}
