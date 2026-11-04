//! Tool 8: text watermark.
//!
//! CJK-safe by construction: the watermark text is rasterized into a
//! transparent bitmap with the system TrueType font (via `engine::ttf` +
//! `engine::raster`) and stamped onto every page as an image. PDF text
//! drawing is deliberately avoided — simple PDF fonts are WinAnsi-encoded
//! and turn every CJK character into `?` (a limitation of the embedded
//! `medpdf` text API), while composite Type0/CID embedding is out of scope
//! for the dependency. A bitmap watermark renders identically in every
//! viewer and keeps the chosen color, size, rotation and opacity.

use medpdf_image::{add_image, DrawImageParams, ImageData};

use crate::engine::{pdf_io, render, validate};
use crate::model::{ToolError, ToolId, ToolOutcome, WatermarkPlacement};
use crate::tools::{
    check_cancel,
    split::{ensure_pdf, single_input},
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

/// Pixels rendered per PDF point (4x supersampling keeps text crisp at
/// print resolutions without blowing up memory).
const SS: f32 = 4.0;

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

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        progress.report(0.05, "渲染水印文字".to_string());
        let (image, wm_w, wm_h) = render_text_bitmap(&text, font_size, color)?;

        let mut doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
        let total = pages.len();

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

            let positions: Vec<(f32, f32)> = match placement {
                WatermarkPlacement::Center => vec![(
                    media[0] + (vis_w - wm_w) / 2.0,
                    media[1] + (vis_h - wm_h) / 2.0,
                )],
                WatermarkPlacement::Tiled => {
                    tiled_positions(vis_w, vis_h, wm_w, wm_h, media[0], media[1])
                }
            };

            for (x, y) in positions {
                let params = DrawImageParams::new(image.clone(), x, y, wm_w, wm_h)
                    .rotation(rotation)
                    .alpha(opacity.clamp(0.0, 1.0))
                    .layer_over(true);
                add_image(&mut doc, *page_id, params).map_err(|e| ToolError::Internal {
                    detail: format!("第 {page_num} 页添加水印失败：{e}"),
                })?;
            }
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &out, req.conflict)?;

        validate::assert_page_count(&out, total as u32, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已在 {total} 页添加文字水印"),
            vec![out],
        ))
    }
}

/// Grid positions for tiled mode, spacing by the bitmap's point size.
fn tiled_positions(
    width: f32,
    height: f32,
    wm_w: f32,
    wm_h: f32,
    origin_x: f32,
    origin_y: f32,
) -> Vec<(f32, f32)> {
    let step_x = (wm_w * 1.5).max(60.0);
    let step_y = (wm_h * 2.0).max(60.0);
    let cols = (width / step_x).ceil().max(1.0) as usize;
    let rows = (height / step_y).ceil().max(1.0) as usize;
    let mut out = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            out.push((
                origin_x + col as f32 * step_x,
                origin_y + row as f32 * step_y,
            ));
        }
    }
    out
}

/// Rasterize `text` into a transparent RGBA watermark bitmap.
///
/// Returns the image plus its size in PDF points (pixels / [`SS`]).
/// Anti-aliasing comes from the scanline rasterizer: glyph coverage is
/// blended over black and re-used as the alpha channel, so edges fade out
/// exactly like the filled glyphs would.
fn render_text_bitmap(
    text: &str,
    font_size: f32,
    color: [u8; 3],
) -> Result<(ImageData, f32, f32), ToolError> {
    let font_set = render::FontSet::system();
    let font = font_set
        .primary()
        .ok_or_else(|| ToolError::UnsupportedFormat {
            path: None,
            detail: "未找到可用的系统 TrueType 字体（C:\\Windows\\Fonts），无法渲染水印文字"
                .to_string(),
        })?;

    let upem = font.units_per_em();
    let scale = font_size / upem;

    // Lay out the glyphs in point space (baseline at y = 0, y up).
    struct PlacedGlyph {
        contours: Vec<Vec<(f32, f32)>>,
    }
    let mut placed: Vec<PlacedGlyph> = Vec::new();
    let mut pen_x = 0.0f32;
    for ch in text.chars() {
        let advance_pt = font.advance(font.glyph_for_char(ch)) * scale;
        if ch != ' ' {
            let gid = font.glyph_for_char(ch);
            let raw = font.outline(gid);
            let contours = raw
                .into_iter()
                .map(|contour| {
                    contour
                        .into_iter()
                        .map(|(x, y)| (pen_x + x * scale, y * scale))
                        .collect()
                })
                .collect();
            placed.push(PlacedGlyph { contours });
        }
        pen_x += advance_pt;
    }
    if placed.is_empty() {
        return Err(ToolError::UnsupportedFormat {
            path: None,
            detail: "水印文字没有可渲染的字符".to_string(),
        });
    }

    // Bounding box in point space.
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for glyph in &placed {
        for contour in &glyph.contours {
            for &(x, y) in contour {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return Err(ToolError::UnsupportedFormat {
            path: None,
            detail: "水印文字没有可渲染的轮廓".to_string(),
        });
    }

    let w_px = ((max_x - min_x) * SS).ceil() as u32 + 2;
    let h_px = ((max_y - min_y) * SS).ceil() as u32 + 2;

    // Fill on a black background; the rendered color channel then doubles as
    // the anti-aliased coverage, which becomes the bitmap's alpha channel.
    let mut pixmap = crate::engine::raster::Pixmap::new(w_px, h_px, [0, 0, 0]);
    for glyph in &placed {
        let mut path = crate::engine::raster::Path::new();
        for contour in &glyph.contours {
            let mut first = true;
            for &(x, y) in contour {
                // Point space -> pixel space (flip y so text reads upright).
                let px = (x - min_x) * SS + 1.0;
                let py = h_px as f32 - ((y - min_y) * SS + 1.0);
                if first {
                    path.move_to(px, py);
                    first = false;
                } else {
                    path.line_to(px, py);
                }
            }
            path.close();
        }
        crate::engine::raster::fill(
            &mut pixmap,
            &path,
            [
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
                1.0,
            ],
        );
    }

    // Split the rendered raster into a solid tint + coverage alpha.
    // `Pixmap::data` is RGBA; filled over black, the RGB channels carry the
    // anti-aliased coverage of the single tint color.
    let data = pixmap.data;
    let mut pixels = Vec::with_capacity((w_px * h_px * 3) as usize);
    let mut alpha = Vec::with_capacity((w_px * h_px) as usize);
    for px in data.chunks_exact(4) {
        let coverage = px[0].max(px[1]).max(px[2]);
        pixels.push(color[0]);
        pixels.push(color[1]);
        pixels.push(color[2]);
        alpha.push(coverage);
    }

    let image = ImageData::Decoded {
        pixels,
        alpha_channel: Some(alpha),
        pixel_width: w_px,
        pixel_height: h_px,
        components: 3,
    };
    Ok((image, w_px as f32 / SS, h_px as f32 / SS))
}
