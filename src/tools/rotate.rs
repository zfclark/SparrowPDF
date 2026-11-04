//! Tool 4: rotate pages by 90/180/270 degrees.
//!
//! Rotation is cumulative: a page already at 90 rotated by 90 lands at 180.

use crate::engine::{pdf_io, validate};
use crate::model::{RotationDegrees, ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel,
    split::{ensure_not_empty, ensure_pdf, single_input},
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct Rotate;

impl Tool for Rotate {
    fn id(&self) -> ToolId {
        ToolId::Rotate
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::Rotate { pages, .. } = &req.options {
            if pages.trim().is_empty() {
                return Err(ToolError::InvalidPageRange {
                    detail: "请填写页码或 all".to_string(),
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
        let (spec, degrees) = match &req.options {
            ToolOptions::Rotate { pages, degrees } => (pages.clone(), *degrees),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        let mut doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&doc);
        // `parse_pages("all", 0)` yields an empty list, so an empty document
        // would otherwise "succeed" while rotating nothing.
        ensure_not_empty(total, path)?;
        let targets = pdf_io::parse_pages(&spec, total)?;
        let page_map = doc.get_pages();

        let delta: u32 = match degrees {
            RotationDegrees::Deg90 => 90,
            RotationDegrees::Deg180 => 180,
            RotationDegrees::Deg270 => 270,
        };
        let mut expected: Vec<(u32, u32)> = Vec::with_capacity(targets.len());

        for (index, page_num) in targets.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / targets.len() as f32,
                format!("旋转第 {page_num} 页"),
            );

            let page_id = *page_map
                .get(page_num)
                .ok_or_else(|| ToolError::InvalidPageRange {
                    detail: format!("第 {page_num} 页不存在"),
                })?;
            let current = pdf_io::page_rotation(&doc, page_id);
            let next = (current + delta) % 360;
            medpdf::set_page_rotation(&mut doc, page_id, next).map_err(|e| {
                ToolError::Internal {
                    detail: format!("设置第 {page_num} 页旋转失败：{e}"),
                }
            })?;
            expected.push((*page_num, next));
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &out, req.conflict)?;

        validate::assert_page_count(&out, total, None)?;
        validate::assert_rotations(&out, &expected)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已旋转 {} 页，每页 +{delta}°", expected.len()),
            vec![out],
        ))
    }
}
