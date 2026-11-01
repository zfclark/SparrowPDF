//! Tool 8: text watermark.
//!
//! The watermark font is resolved from the operating system (a CJK-capable
//! family when available, otherwise a PDF built-in font). No font is embedded in
//! the binary — that is what keeps the executable under the size budget.

use std::path::Path;

use medpdf::{
    AddTextParams, EmbeddedFontCache, FontCache, FontData, HAlign, PdfColor, VAlign,
    add_text_params, find_font, set_page_rotation,
};

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome, WatermarkPlacement};
use crate::tools::{
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest, check_cancel,
    split::{ensure_pdf, single_input},
};

/// Family names tried in order; the first one the OS resolves wins.
const FONT_CANDIDATES: &[&str] = &[
    "Microsoft YaHei",
    "Microsoft YaHei UI",
    "DengXian",
    "SimHei",
    "SimSun",
    "PingFang SC",
    "Heiti SC",
    "Songti SC",
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "DejaVu Sans",
];

/// Last resort: a PDF built-in font (ASCII only, no embedding required).
const BUILTIN_FALLBACK: &str = "@Helvetica";

pub struct TextWatermark;

impl Tool for TextWatermark {
    fn id(&self) -> ToolId {
        ToolId::TextWatermark
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        if let ToolOptions::TextWatermark { text, .. } = &req.options {
            if text.trim().is_empty() {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: "水印文字不能为空".to_string(),
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
        let (text, font_size, color, opacity, rotation, placement) = match &req.options {
            ToolOptions::TextWatermark {
                text,
                font_size,
                color,
                opacity,
                rotation,
                placement,
            } => (
                text.clone(),
                *font_size,
                *color,
                *opacity,
                *rotation,
                *placement,
            ),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let (font_data, font_name) = resolve_font(&text)?;

        let mut doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
        let total = pages.len();

        let mut font_cache = EmbeddedFontCache::new();
        let tint = PdfColor::rgba(
            color[0] as f32 / 255.0,
            color[1] as f32 / 255.0,
            color[2] as f32 / 255.0,
            opacity.clamp(0.0, 1.0),
        );

        for (index, (page_num, page_id)) in pages.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("添加文字水印 {}/{}", page_num, total),
            );

            let media = pdf_io::page_media_box(&doc, *page_id);
            let page_w = media[2] - media[0];
            let page_h = media[3] - media[1];
            // /Rotate changes the visual box; swap so the watermark stays readable.
            let rotated = matches!(pdf_io::page_rotation(&doc, *page_id), 90 | 270);
            let (vis_w, vis_h) = if rotated {
                (page_h, page_w)
            } else {
                (page_w, page_h)
            };

            let placements: Vec<(f32, f32)> = match placement {
                WatermarkPlacement::Center => vec![(vis_w / 2.0, vis_h / 2.0)],
                WatermarkPlacement::Tiled => tiled_positions(vis_w, vis_h, font_size, &text),
            };

            for (cx, cy) in placements {
                let mut params = AddTextParams::new(
                    text.clone(),
                    font_data.clone(),
                    font_name.clone(),
                )
                .font_size(font_size)
                .color(tint)
                .rotation(rotation)
                .h_align(HAlign::Center)
                .v_align(VAlign::Center)
                .layer_over(true);
                // `page_media_box` origin may be non-zero; shift into media space.
                params.x = cx + media[0];
                params.y = cy + media[1];

                add_text_params(&mut doc, *page_id, &params, &mut font_cache).map_err(|e| {
                    ToolError::Internal {
                        detail: format!("第 {page_num} 页添加水印失败：{e}"),
                    }
                })?;
            }

            // `set_page_rotation` is a no-op here; it exists so the invariant is
            // explicit: watermarking must not change page rotation.
            let keep = pdf_io::page_rotation(&doc, *page_id);
            set_page_rotation(&mut doc, *page_id, keep).map_err(|e| ToolError::Internal {
                detail: format!("保持第 {page_num} 页旋转失败：{e}"),
            })?;
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &req.output)?;

        validate::assert_page_count(&req.output, total as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已在 {total} 页添加文字水印"),
            vec![req.output.clone()],
        ))
    }
}

/// Positions for the tiled mode: a grid spaced by text extents.
fn tiled_positions(width: f32, height: f32, font_size: f32, text: &str) -> Vec<(f32, f32)> {
    let approx_width = (text.chars().count() as f32 * font_size * 0.7).max(font_size * 2.0);
    let step_x = approx_width.max(60.0);
    let step_y = (font_size * 3.0).max(60.0);

    let cols = (width / step_x).ceil().max(1.0) as usize;
    let rows = (height / step_y).ceil().max(1.0) as usize;

    let mut out = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            out.push((
                (col as f32 + 0.5) * step_x,
                (row as f32 + 0.5) * step_y,
            ));
        }
    }
    out
}

/// Resolve a usable font: an OS CJK family when present, else a built-in.
fn resolve_font(text: &str) -> Result<(FontData, String), ToolError> {
    let mut cache = FontCache::new();

    if needs_cjk(text) {
        for family in FONT_CANDIDATES {
            let Ok(resolved) = find_font(Path::new(family)) else {
                continue;
            };
            // `find_font` also succeeds for `FontPath::Hack`/`BuiltIn`; only a
            // real file/memory font can carry CJK glyphs.
            match &resolved {
                medpdf::FontPath::Path(_) | medpdf::FontPath::Memory(_, _) => {}
                _ => continue,
            }
            let name = resolved.get_name();
            if let Ok(data) = cache.get_data(&resolved) {
                return Ok((data, name));
            }
        }
        return Err(ToolError::UnsupportedFormat {
            path: None,
            detail: "系统中未找到可嵌入的中文字体，请安装微软雅黑、思源黑体或 Noto Sans CJK 后重试"
                .to_string(),
        });
    }

    // ASCII-only watermark can use a built-in font, with no embedding at all.
    if let Ok(resolved) = find_font(Path::new(BUILTIN_FALLBACK)) {
        let name = resolved.get_name();
        if let Ok(data) = cache.get_data(&resolved) {
            return Ok((data, name));
        }
    }

    Ok((
        FontData::BuiltIn("Helvetica".to_string()),
        "Helvetica".to_string(),
    ))
}

/// True when the text contains characters outside Latin-1.
fn needs_cjk(text: &str) -> bool {
    text.chars().any(|c| !c.is_ascii() && c as u32 > 0xFF)
}
