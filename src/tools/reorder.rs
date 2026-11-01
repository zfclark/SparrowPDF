//! Tool 6: reorder pages.
//!
//! The UI keeps a drag-to-reorder list of 0-based source indices; this tool
//! rebuilds the document in that order. `egui_dnd` is used by the UI only,
//! never here.

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest, check_cancel,
    split::{ensure_pdf, single_input},
};

pub struct Reorder;

impl Tool for Reorder {
    fn id(&self) -> ToolId {
        ToolId::Reorder
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::Reorder { order } = &req.options {
            if order.is_empty() {
                return Err(ToolError::InvalidPageRange {
                    detail: "页序为空".to_string(),
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
        let order = match &req.options {
            ToolOptions::Reorder { order } => order.clone(),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let source = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&source);

        // 0-based source indices in, 1-based page numbers out; reject duplicates
        // and out-of-range indices so the output assertion stays meaningful.
        let mut seen = vec![false; total as usize];
        let mut pages: Vec<u32> = Vec::with_capacity(order.len());
        for index in &order {
            if *index >= total as usize {
                return Err(ToolError::InvalidPageRange {
                    detail: format!("页序包含越界索引 {}（共 {total} 页）", index + 1),
                });
            }
            if seen[*index] {
                return Err(ToolError::InvalidPageRange {
                    detail: format!("页序包含重复的第 {} 页", index + 1),
                });
            }
            seen[*index] = true;
            pages.push(*index as u32 + 1);
        }

        progress.report(0.2, "重建页面顺序".to_string());
        check_cancel(cancel)?;

        let mut dest = pdf_io::new_document();
        pdf_io::copy_pages(&mut dest, &source, &pages)?;

        progress.report(0.9, "写入 PDF".to_string());
        pdf_io::save_document(&mut dest, &req.output)?;
        validate::assert_page_count(&req.output, pages.len() as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已重排 {} 页", pages.len()),
            vec![req.output.clone()],
        ))
    }
}
