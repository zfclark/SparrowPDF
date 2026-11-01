//! Tool 7: extract pages into a new PDF.

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest, check_cancel,
    split::{ensure_pdf, single_input},
};

pub struct Extract;

impl Tool for Extract {
    fn id(&self) -> ToolId {
        ToolId::Extract
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::Extract { pages } = &req.options {
            if pages.trim().is_empty() {
                return Err(ToolError::InvalidPageRange {
                    detail: "请填写要提取的页码".to_string(),
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
            ToolOptions::Extract { pages } => pages.clone(),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let source = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&source);
        let pages = pdf_io::parse_pages(&spec, total)?;

        progress.report(0.3, "复制所选页面".to_string());
        check_cancel(cancel)?;

        let mut dest = pdf_io::new_document();
        pdf_io::copy_pages(&mut dest, &source, &pages)?;

        progress.report(0.9, "写入 PDF".to_string());
        pdf_io::save_document(&mut dest, &req.output)?;
        validate::assert_page_count(&req.output, pages.len() as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已提取 {} 页", pages.len()),
            vec![req.output.clone()],
        ))
    }
}
