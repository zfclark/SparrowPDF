//! Tool 11: PDF to images, using the pure-Rust rasterizer.
//!
//! Priority per the spec: oxidize-pdf, then oxpdf-render, then lopdf + a manual
//! rasterizer. Neither `oxidize-pdf` (which documents that page rasterization
//! needs an external renderer) nor `oxpdf-render` (not published) can rasterize,
//! so the manual path in `engine::render` is used. If that fails, the tool
//! returns an internal error — it never falls back to PDFium.

use std::path::PathBuf;

use crate::engine::{pdf_io, render, validate};
use crate::model::{ImageFormat, ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest, check_cancel, ensure_dir,
    split::{ensure_pdf, single_input},
};

pub struct PdfToImage;

impl Tool for PdfToImage {
    fn id(&self) -> ToolId {
        ToolId::PdfToImage
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::PdfToImage { dpi, .. } = &req.options {
            if !(36..=600).contains(dpi) {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: format!("DPI 需在 36-600 之间，收到 {dpi}"),
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
        let (dpi, format) = match &req.options {
            ToolOptions::PdfToImage { dpi, format } => (*dpi, *format),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
        if pages.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "文档没有页面".to_string(),
            });
        }

        let fonts = render::FontSet::system();
        if fonts.is_empty() {
            // Text will be omitted, but vector content still renders; say so once.
            progress.report(0.0, "未找到系统字体，文字内容将被跳过".to_string());
        }

        let out_dir = output_dir(req);
        ensure_dir(&out_dir)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string());

        let mut outputs: Vec<PathBuf> = Vec::with_capacity(pages.len());

        for (index, (page_num, page_id)) in pages.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / pages.len() as f32,
                format!("渲染第 {page_num}/{} 页", pages.len()),
            );

            let pixmap = render::render_page(&doc, *page_id, dpi, &fonts).map_err(|e| {
                ToolError::Internal {
                    detail: format!("第 {page_num} 页渲染失败：{e}"),
                }
            })?;

            let (width, height) = (pixmap.width, pixmap.height);
            let bytes = match format {
                ImageFormat::Png => pixmap.to_png(),
                ImageFormat::Jpeg => pixmap.to_jpeg(),
            }
            .map_err(|e| ToolError::Internal {
                detail: format!("第 {page_num} 页编码失败：{e}"),
            })?;

            let out = out_dir.join(format!("{stem}_p{page_num:03}.{}", format.extension()));
            pdf_io::write_file(&out, &bytes)?;
            validate::assert_image(&out, width, height)?;
            outputs.push(out);
        }

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已导出 {} 张 {} 图片（{dpi} DPI）", outputs.len(), format.label()),
            outputs,
        ))
    }
}

fn output_dir(req: &ToolRequest) -> PathBuf {
    let out = &req.output;
    let has_image_ext = out
        .extension()
        .map(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            e == "png" || e == "jpg" || e == "jpeg"
        })
        .unwrap_or(false);
    if has_image_ext {
        return out
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    if out.as_os_str().is_empty() {
        return req
            .inputs
            .first()
            .and_then(|i| i.path.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    out.clone()
}
