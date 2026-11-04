//! Tool 9: image watermark.

use std::path::PathBuf;

use medpdf_image::{add_image, load_image, DrawImageParams, ImageFit};

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome, WatermarkPlacement};
use crate::tools::{
    check_cancel,
    split::{ensure_pdf, single_input},
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct ImageWatermark;

impl Tool for ImageWatermark {
    fn id(&self) -> ToolId {
        ToolId::ImageWatermark
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        let pdf = single_input(req)?;
        ensure_pdf(pdf)?;
        let Some(watermark) = req.inputs.get(1) else {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请选择一张作为水印的图片".to_string(),
            });
        };
        if !watermark.path.is_file() {
            return Err(ToolError::ReadFailed {
                path: watermark.path.clone(),
                detail: "水印图片不存在".to_string(),
            });
        }
        let ext = watermark
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            return Err(ToolError::UnsupportedFormat {
                path: Some(watermark.path.clone()),
                detail: format!("水印图片仅支持 JPEG / PNG，收到 .{ext}"),
            });
        }
        if let ToolOptions::ImageWatermark { scale, opacity, .. } = &req.options {
            if !(0.0 < *scale && *scale <= 1.0) {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: "缩放比例需在 (0, 1] 区间".to_string(),
                });
            }
            if !(0.0..=1.0).contains(opacity) {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: "透明度需在 [0, 1] 区间".to_string(),
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
        let pdf_path = single_input(req)?;
        // `validate` guarantees the second slot, but `execute` must not panic if
        // it is ever called without it (worker catches panics, yet an
        // "内部错误" is a poor answer for a missing file).
        let watermark_path: &PathBuf = &req
            .inputs
            .get(1)
            .ok_or_else(|| ToolError::UnsupportedFormat {
                path: None,
                detail: "请选择一张作为水印的图片".to_string(),
            })?
            .path;
        let (scale, opacity, position) = match &req.options {
            ToolOptions::ImageWatermark {
                scale,
                opacity,
                position,
            } => (*scale, *opacity, *position),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        let mut doc = pdf_io::open_for_edit(pdf_path, req.password.as_deref())?;
        let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
        let total = pages.len();

        for (index, (page_num, page_id)) in pages.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("添加图片水印 {}/{}", page_num, total),
            );

            // Re-load per page: `add_image` consumes the image data.
            let image = load_image(watermark_path).map_err(|e| ToolError::UnsupportedFormat {
                path: Some(watermark_path.to_path_buf()),
                detail: e.to_string(),
            })?;

            let media = pdf_io::page_media_box(&doc, *page_id);
            let page_w = media[2] - media[0];
            let page_h = media[3] - media[1];
            let rotated = matches!(pdf_io::page_rotation(&doc, *page_id), 90 | 270);
            let (vis_w, vis_h) = if rotated {
                (page_h, page_w)
            } else {
                (page_w, page_h)
            };

            let wm_w = vis_w * scale;
            let wm_h = vis_h * scale;

            let positions: Vec<(f32, f32)> = match position {
                WatermarkPlacement::Center => vec![(
                    media[0] + (vis_w - wm_w) / 2.0,
                    media[1] + (vis_h - wm_h) / 2.0,
                )],
                WatermarkPlacement::Tiled => {
                    let mut out = Vec::new();
                    let step_x = wm_w.max(1.0);
                    let step_y = wm_h.max(1.0);
                    let cols = (vis_w / step_x).ceil().max(1.0) as usize;
                    let rows = (vis_h / step_y).ceil().max(1.0) as usize;
                    for row in 0..rows {
                        for col in 0..cols {
                            out.push((
                                media[0] + col as f32 * step_x,
                                media[1] + row as f32 * step_y,
                            ));
                        }
                    }
                    out
                }
            };

            for (x, y) in positions {
                let params = DrawImageParams::new(image.clone(), x, y, wm_w, wm_h)
                    .fit(ImageFit::Contain)
                    .alpha(opacity)
                    .max_dpi(0.0)
                    .layer_over(true);
                add_image(&mut doc, *page_id, params).map_err(|e| ToolError::Internal {
                    detail: format!("第 {page_num} 页添加图片水印失败：{e}"),
                })?;
            }
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &out, req.conflict)?;

        validate::assert_page_count(&out, total as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已在 {total} 页添加图片水印"),
            vec![out],
        ))
    }
}
