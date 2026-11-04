//! Tool 6: reorder pages.
//!
//! The UI keeps a drag-to-reorder list of 0-based source indices; this tool
//! rebuilds the document in that order. `egui_dnd` is used by the UI only,
//! never here.

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel,
    split::{ensure_not_empty, ensure_pdf, single_input},
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct Reorder;

/// Translate a 0-based source order into 1-based page numbers.
///
/// Shared by the tool and its live preview so the strip the user sees and the
/// page sequence written to disk can never disagree. Out-of-range indices are
/// dropped here; [`Reorder::execute`] rejects them with a message first.
pub fn ordered_pages(order: &[usize], total: u32) -> Vec<u32> {
    order
        .iter()
        .filter(|index| **index < total as usize)
        .map(|index| *index as u32 + 1)
        .collect()
}

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

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        let source = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&source);
        ensure_not_empty(total, path)?;

        // 0-based source indices in, 1-based page numbers out; reject duplicates
        // and out-of-range indices so the output assertion stays meaningful.
        let mut seen = vec![false; total as usize];
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
        }
        let pages = ordered_pages(&order, total);

        progress.report(0.2, "重建页面顺序".to_string());
        check_cancel(cancel)?;

        let mut dest = pdf_io::new_document();
        pdf_io::copy_pages(&mut dest, &source, &pages)?;

        progress.report(0.9, "写入 PDF".to_string());
        pdf_io::save_document(&mut dest, &out, req.conflict)?;
        validate::assert_page_count(&out, pages.len() as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已重排 {} 页", pages.len()),
            vec![out],
        ))
    }
}
