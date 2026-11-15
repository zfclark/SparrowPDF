//! Text watermarks, rectangles, and lines — drawing operations on PDF pages.
//!
//! Supports color, alpha, rotation, horizontal/vertical alignment, underline,
//! strikeout, and layering (over or under existing content).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::{MedpdfError, Result};
use crate::font_helpers;
use crate::pdf_helpers::{KEY_CONTENTS, KEY_EXTGSTATE, KEY_FONT, KEY_FONT_DESCRIPTOR};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, ObjectId, Stream, StringFormat};

/// Per-font tracking entry for subsetting support.
pub struct CachedFontEntry {
    pub(crate) font_id: ObjectId,
    pub(crate) font_key: String,
    pub(crate) font_stream_id: ObjectId,
    pub(crate) descriptor_id: ObjectId,
    pub(crate) data: Arc<Vec<u8>>,
    pub(crate) used_chars: HashSet<char>,
}

/// Cache for embedded font PDF objects, preventing duplicate font embedding.
///
/// Keys on `Arc::as_ptr()` identity — safe because [`FontCache`](crate::pdf_font::FontCache)
/// guarantees the same `Arc<Vec<u8>>` for the same font path. Stores font dictionary IDs,
/// stream IDs, and character usage for post-watermark subsetting.
pub struct EmbeddedFontCache {
    cache: HashMap<usize, CachedFontEntry>,
}

impl EmbeddedFontCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    fn get(&self, data: &Arc<Vec<u8>>) -> Option<&CachedFontEntry> {
        self.cache.get(&(Arc::as_ptr(data) as usize))
    }

    fn insert(
        &mut self,
        data: &Arc<Vec<u8>>,
        font_id: ObjectId,
        font_key: String,
        font_stream_id: ObjectId,
        descriptor_id: ObjectId,
    ) {
        self.cache.insert(
            Arc::as_ptr(data) as usize,
            CachedFontEntry {
                font_id,
                font_key,
                font_stream_id,
                descriptor_id,
                data: Arc::clone(data),
                used_chars: HashSet::new(),
            },
        );
    }

    fn record_chars(&mut self, data: &Arc<Vec<u8>>, text: &str) {
        if let Some(entry) = self.cache.get_mut(&(Arc::as_ptr(data) as usize)) {
            entry.used_chars.extend(text.chars());
        }
    }

    /// Iterates over all cached embedded font entries.
    pub fn embedded_entries(&self) -> impl Iterator<Item = &CachedFontEntry> {
        self.cache.values()
    }
}

impl Default for EmbeddedFontCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts the net q/Q balance across content streams.
/// Returns the number of unclosed 'q' operations (positive means more q's than Q's).
fn count_q_balance(dest_doc: &Document, content_refs: &[ObjectId]) -> Result<isize> {
    let mut total_balance: isize = 0;

    for &content_id in content_refs {
        let obj = dest_doc.get_object(content_id)?;
        if let Ok(stream) = obj.as_stream() {
            // Need to decompress if necessary
            let content_bytes = if stream.is_compressed() {
                std::borrow::Cow::Owned(stream.decompressed_content()?)
            } else {
                std::borrow::Cow::Borrowed(&stream.content)
            };

            // Parse and count q/Q operations
            match Content::decode(&content_bytes) {
                Ok(content) => {
                    for operation in content.operations.iter() {
                        match operation.operator.as_str() {
                            "q" => total_balance += 1,
                            "Q" => total_balance -= 1,
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to decode content stream {content_id:?}: {e}; assuming balanced q/Q");
                }
            }
        }
    }

    Ok(total_balance)
}

/// Gets all content stream ObjectIds from a page's Contents entry.
fn get_content_stream_ids(dest_doc: &Document, page_id: ObjectId) -> Result<Vec<ObjectId>> {
    let page_dict = dest_doc.get_object(page_id)?.as_dict()?;

    match page_dict.get(KEY_CONTENTS) {
        Ok(Object::Array(arr)) => Ok(arr
            .iter()
            .filter_map(|obj| obj.as_reference().ok())
            .collect()),
        Ok(Object::Reference(id)) => Ok(vec![*id]),
        Ok(_) => Err(MedpdfError::new("Unexpected Contents type")),
        Err(_) => Ok(vec![]), // No contents yet
    }
}

/// Converts a UTF-8 string to WinAnsiEncoding (Windows Code Page 1252) bytes.
/// Characters that cannot be represented in WinAnsiEncoding are replaced with '?'.
pub(crate) fn utf8_to_winansi(text: &str) -> Vec<u8> {
    text.chars().map(font_helpers::unicode_to_winansi).collect()
}

// Re-export for tests in this module.
#[cfg(test)]
use font_helpers::unicode_to_winansi;

/// Measures text width using an already-parsed font face, avoiding redundant parsing.
fn measure_text_width_with_face(face: &ttf_parser::Face, font_size: f32, text: &str) -> f32 {
    let upem = face.units_per_em() as f32;
    if upem == 0.0 {
        return 0.0;
    }
    let scale = font_size / upem;
    let mut width: f32 = 0.0;
    for ch in text.chars() {
        if let Some(glyph_id) = face.glyph_index(ch) {
            width += face.glyph_hor_advance(glyph_id).unwrap_or(0) as f32;
        }
    }
    width * scale
}

fn add_font_objects(
    dest_doc: &mut Document,
    page_id: (u32, u16),
    font_data: &crate::font_data::FontData,
    font_name: &str,
    font_cache: &mut EmbeddedFontCache,
) -> Result<String> {
    match font_data {
        crate::font_data::FontData::Hack(n) => Ok(format!("F{n}")),
        crate::font_data::FontData::BuiltIn(_) => add_known_named_font(dest_doc, page_id, font_name),
        crate::font_data::FontData::Embedded(data) => {
            if let Some(entry) = font_cache.get(data) {
                // Font already embedded — just register it in this page's resources
                register_font_in_page_resources(dest_doc, page_id, entry.font_id)?;
                Ok(entry.font_key.clone())
            } else {
                let (font_id, font_key, font_stream_id, descriptor_id) =
                    add_embedded_font(dest_doc, page_id, data)?;
                font_cache.insert(data, font_id, font_key.clone(), font_stream_id, descriptor_id);
                Ok(font_key)
            }
        }
    }
}

/// Registers a font object in the page's resources and returns the font key.
fn register_font_in_page_resources(
    dest_doc: &mut Document,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<String> {
    let font_key = format!("F{}", font_id.0);
    crate::pdf_helpers::register_in_page_resources(
        dest_doc, page_id, KEY_FONT, font_key.as_bytes(), font_id,
    )?;
    Ok(font_key)
}

/// Registers an ExtGState object in the page's resources and returns the gs key.
pub fn register_extgstate_in_page_resources(
    dest_doc: &mut Document,
    page_id: ObjectId,
    gs_id: ObjectId,
) -> Result<String> {
    let gs_key = format!("GS{}", gs_id.0);
    crate::pdf_helpers::register_in_page_resources(
        dest_doc, page_id, KEY_EXTGSTATE, gs_key.as_bytes(), gs_id,
    )?;
    Ok(gs_key)
}

fn add_known_named_font(
    dest_doc: &mut Document,
    page_id: ObjectId,
    font_name: &str,
) -> Result<String> {
    let font_dict = dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => font_name.to_string(),
        "Encoding" => "WinAnsiEncoding",
    };
    let font_id = dest_doc.add_object(font_dict);
    register_font_in_page_resources(dest_doc, page_id, font_id)
}

fn widths_as_object_array(widths: &[u16]) -> Object {
    Object::Array(widths.iter().map(|v| Object::Integer(*v as i64)).collect())
}

fn bbox_as_object_array(bbox: &[i16]) -> Result<Object> {
    if bbox.len() != 4 {
        return Err(MedpdfError::new("FontBBox must have exactly 4 elements"));
    }
    Ok(Object::Array(vec![
        Object::Integer(bbox[0] as i64),
        Object::Integer(bbox[1] as i64),
        Object::Integer(bbox[2] as i64),
        Object::Integer(bbox[3] as i64),
    ]))
}

struct TextMetrics {
    text_width: f32,
    dx: f32,
    dy: f32,
}

/// Computes text width and alignment offsets from font metrics.
fn compute_text_metrics(params: &crate::types::AddTextParams) -> TextMetrics {
    let needs_width = params.h_align != crate::types::HAlign::Left
        || params.v_align != crate::types::VAlign::Baseline
        || params.strikeout
        || params.underline;
    let face_opt = params.font_data.embedded_bytes()
        .and_then(|bytes| ttf_parser::Face::parse(bytes, 0).ok());

    let text_width = if needs_width {
        match &face_opt {
            Some(face) => measure_text_width_with_face(face, params.font_size, &params.text),
            None => params.text.len() as f32 * params.font_size * 0.6,
        }
    } else {
        0.0
    };

    let dx = match params.h_align {
        crate::types::HAlign::Left => 0.0,
        crate::types::HAlign::Center => -text_width / 2.0,
        crate::types::HAlign::Right => -text_width,
    };

    // Compute vertical metrics from font data (scaled to font_size).
    // For built-in/hack fonts, use reasonable approximations.
    let approx = (params.font_size * 0.7, params.font_size * -0.3, params.font_size * 0.5, params.font_size * 0.7, params.font_size * -0.3);
    let (ascent, descent, x_height, cap_height, bbox_bottom) =
        if let Some(face) = &face_opt {
            let upem = face.units_per_em() as f32;
            if upem > 0.0 {
                let scale = params.font_size / upem;
                (
                    face.ascender() as f32 * scale,
                    face.descender() as f32 * scale,
                    face.x_height().unwrap_or((upem * 0.5) as i16) as f32 * scale,
                    face.capital_height().unwrap_or(face.ascender()) as f32 * scale,
                    face.global_bounding_box().y_min as f32 * scale,
                )
            } else {
                approx
            }
        } else {
            approx
        };
    let dy = match params.v_align {
        crate::types::VAlign::Baseline => 0.0,
        crate::types::VAlign::DescentBottom => -descent,
        crate::types::VAlign::Bottom => -bbox_bottom,
        crate::types::VAlign::Center => -x_height / 2.0,
        crate::types::VAlign::Top => -ascent,
        crate::types::VAlign::CapTop => -cap_height,
    };

    TextMetrics { text_width, dx, dy }
}

/// Pushes alpha transparency operations via ExtGState when not fully opaque.
fn push_alpha_ops(
    ops: &mut Vec<Operation>,
    dest_doc: &mut Document,
    page_id: ObjectId,
    alpha: f32,
) -> Result<()> {
    if (alpha - 1.0).abs() > f32::EPSILON {
        let gs_dict = dictionary! {
            "Type" => "ExtGState",
            "ca" => alpha,
            "CA" => alpha,
        };
        let gs_id = dest_doc.add_object(gs_dict);
        let gs_key = register_extgstate_in_page_resources(dest_doc, page_id, gs_id)?;
        ops.push(Operation::new(
            "gs",
            vec![Object::Name(gs_key.as_bytes().to_vec())],
        ));
    }
    Ok(())
}

/// Builds the PDF operations for text rendering (color, alpha, rotation, text placement).
fn build_text_ops(
    dest_doc: &mut Document,
    page_id: ObjectId,
    params: &crate::types::AddTextParams,
    font_key: &str,
    metrics: &TextMetrics,
) -> Result<Vec<Operation>> {
    let encoded_text = utf8_to_winansi(&params.text);
    let mut ops = vec![Operation::new("q", vec![])];
    let color = params.color.clamped();

    push_alpha_ops(&mut ops, dest_doc, page_id, color.a)?;

    ops.push(Operation::new(
        "rg",
        vec![color.r.into(), color.g.into(), color.b.into()],
    ));

    let has_rotation = params.rotation.abs() > 0.001;
    if has_rotation {
        let angle = params.rotation.to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        ops.push(Operation::new(
            "cm",
            vec![cos.into(), sin.into(), (-sin).into(), cos.into(), params.x.into(), params.y.into()],
        ));
    }

    ops.push(Operation::new("BT", vec![]));
    ops.push(Operation::new(
        "Tf",
        vec![Object::Name(font_key.as_bytes().to_vec()), params.font_size.into()],
    ));

    let (tx, ty) = if has_rotation {
        (metrics.dx, metrics.dy)
    } else {
        (params.x + metrics.dx, params.y + metrics.dy)
    };
    ops.push(Operation::new("Td", vec![tx.into(), ty.into()]));

    ops.push(Operation::new(
        "Tj",
        vec![Object::String(encoded_text, StringFormat::Literal)],
    ));
    ops.push(Operation::new("ET", vec![]));

    Ok(ops)
}

/// Builds underline/strikeout rectangle operations.
fn build_decoration_ops(
    params: &crate::types::AddTextParams,
    metrics: &TextMetrics,
) -> Vec<Operation> {
    let mut ops = Vec::new();
    if !params.underline && !params.strikeout {
        return ops;
    }
    // In rotated mode, cm is active so we use (dx, dy) offsets.
    // In non-rotated mode, we use absolute (params.x + dx, params.y + dy) coords.
    let has_rotation = params.rotation.abs() > 0.001;
    let rect_x = if has_rotation { metrics.dx } else { params.x + metrics.dx };
    let rect_base_y = if has_rotation { metrics.dy } else { params.y + metrics.dy };
    let line_height = params.font_size * 0.05;
    if params.underline {
        let line_y = rect_base_y - params.font_size * 0.15;
        ops.push(Operation::new(
            "re",
            vec![rect_x.into(), line_y.into(), metrics.text_width.into(), line_height.into()],
        ));
        ops.push(Operation::new("f", vec![]));
    }
    if params.strikeout {
        let line_y = rect_base_y + params.font_size * 0.3;
        ops.push(Operation::new(
            "re",
            vec![rect_x.into(), line_y.into(), metrics.text_width.into(), line_height.into()],
        ));
        ops.push(Operation::new("f", vec![]));
    }
    ops
}

/// Encodes operations into a content stream and inserts it into the page.
fn encode_and_insert(
    dest_doc: &mut Document,
    page_id: ObjectId,
    ops: Vec<Operation>,
    layer_over: bool,
) -> Result<()> {
    let content_id = dest_doc.add_object(Stream::new(
        dictionary! {},
        Content { operations: ops }.encode()?,
    ));
    insert_content_stream(dest_doc, page_id, content_id, layer_over)
}

/// Adds text to a page using rich parameters (color, f32 coords, rotation, alignment).
///
/// Vertical alignment uses real font metrics (ascent, descent, x-height) when
/// font data is available, with reasonable approximations for built-in fonts.
pub fn add_text_params(
    dest_doc: &mut Document,
    page_id: ObjectId,
    params: &crate::types::AddTextParams,
    font_cache: &mut EmbeddedFontCache,
) -> Result<()> {
    let font_key = add_font_objects(dest_doc, page_id, &params.font_data, &params.font_name, font_cache)?;
    if let crate::font_data::FontData::Embedded(ref data) = params.font_data {
        font_cache.record_chars(data, &params.text);
    }
    let metrics = compute_text_metrics(params);
    let mut ops = build_text_ops(dest_doc, page_id, params, &font_key, &metrics)?;
    ops.extend(build_decoration_ops(params, &metrics));
    ops.push(Operation::new("Q", vec![]));
    encode_and_insert(dest_doc, page_id, ops, params.layer_over)
}

/// Inserts a content stream into a page, either over or under existing content.
///
/// When `layer_over` is true, existing content is wrapped in q/Q and new content appended.
/// When `layer_over` is false, new content is prepended before existing content.
pub fn insert_content_stream(
    dest_doc: &mut Document,
    page_id: ObjectId,
    content_id: ObjectId,
    layer_over: bool,
) -> Result<()> {
    let (q_id, closing_q_id) = if layer_over {
        let existing_content_ids = get_content_stream_ids(dest_doc, page_id)?;
        let q_balance = count_q_balance(dest_doc, &existing_content_ids)?;

        let q_stream = Stream::new(dictionary! {}, b"q\n".to_vec());
        let q_id = dest_doc.add_object(q_stream);

        let num_closing_qs = 1 + q_balance.max(0) as usize;
        let closing_content = "Q\n".repeat(num_closing_qs);
        let closing_stream = Stream::new(dictionary! {}, closing_content.into_bytes());
        let closing_q_id = dest_doc.add_object(closing_stream);

        (Some(q_id), Some(closing_q_id))
    } else {
        (None, None)
    };

    let page_dict = dest_doc
        .get_object_mut(page_id)?
        .as_dict_mut()
        .map_err(|_| MedpdfError::new("Page object is not a dictionary"))?;

    if let Ok(contents) = page_dict.get_mut(KEY_CONTENTS) {
        match contents {
            Object::Array(arr) => {
                if layer_over {
                    let q = q_id.ok_or_else(|| MedpdfError::new("Internal error: missing q stream ID"))?;
                    let closing_q = closing_q_id.ok_or_else(|| MedpdfError::new("Internal error: missing closing q stream ID"))?;
                    arr.insert(0, Object::Reference(q));
                    arr.push(Object::Reference(closing_q));
                    arr.push(Object::Reference(content_id));
                } else {
                    arr.insert(0, Object::Reference(content_id));
                }
            }
            Object::Reference(id) => {
                let old_id = *id;
                *contents = if layer_over {
                    let q = q_id.ok_or_else(|| MedpdfError::new("Internal error: missing q stream ID"))?;
                    let closing_q = closing_q_id.ok_or_else(|| MedpdfError::new("Internal error: missing closing q stream ID"))?;
                    Object::Array(vec![
                        Object::Reference(q),
                        Object::Reference(old_id),
                        Object::Reference(closing_q),
                        Object::Reference(content_id),
                    ])
                } else {
                    Object::Array(vec![
                        Object::Reference(content_id),
                        Object::Reference(old_id),
                    ])
                };
            }
            _ => return Err(MedpdfError::new("Unexpected page Contents type")),
        }
    } else {
        page_dict.set(
            KEY_CONTENTS,
            Object::Array(vec![Object::Reference(content_id)]),
        );
    }

    Ok(())
}

/// Draws a filled rectangle on a page.
pub fn add_rect(
    dest_doc: &mut Document,
    page_id: ObjectId,
    params: &crate::types::DrawRectParams,
) -> Result<()> {
    let color = params.color.clamped();
    let mut ops = vec![Operation::new("q", vec![])];

    push_alpha_ops(&mut ops, dest_doc, page_id, color.a)?;

    // Set fill color
    ops.push(Operation::new(
        "rg",
        vec![color.r.into(), color.g.into(), color.b.into()],
    ));

    // Draw rectangle and fill
    ops.push(Operation::new(
        "re",
        vec![
            params.x.into(),
            params.y.into(),
            params.width.into(),
            params.height.into(),
        ],
    ));
    ops.push(Operation::new("f", vec![]));
    ops.push(Operation::new("Q", vec![]));

    encode_and_insert(dest_doc, page_id, ops, params.layer_over)
}

/// Draws a stroked line on a page.
pub fn add_line(
    dest_doc: &mut Document,
    page_id: ObjectId,
    params: &crate::types::DrawLineParams,
) -> Result<()> {
    let color = params.color.clamped();
    let mut ops = vec![Operation::new("q", vec![])];

    push_alpha_ops(&mut ops, dest_doc, page_id, color.a)?;

    // Set stroke color
    ops.push(Operation::new(
        "RG",
        vec![color.r.into(), color.g.into(), color.b.into()],
    ));

    // Set line width
    ops.push(Operation::new("w", vec![params.line_width.into()]));

    // Move to start, line to end, stroke
    ops.push(Operation::new(
        "m",
        vec![params.x1.into(), params.y1.into()],
    ));
    ops.push(Operation::new(
        "l",
        vec![params.x2.into(), params.y2.into()],
    ));
    ops.push(Operation::new("S", vec![]));
    ops.push(Operation::new("Q", vec![]));

    encode_and_insert(dest_doc, page_id, ops, params.layer_over)
}

fn add_embedded_font(
    dest_doc: &mut Document,
    page_id: ObjectId,
    font_data: &[u8],
) -> Result<(ObjectId, String, ObjectId, ObjectId)> {
    let (font_info, font_descriptor) = font_helpers::get_pdf_font_info_of_data(font_data)?;
    let mut font_dict = dictionary! {
        "Type" => "Font",
        "Subtype" =>  font_info.subtype,
        "BaseFont" => font_info.base_font,
        "FirstChar" => font_info.first_char,
        "LastChar" => font_info.last_char,
        "Widths" => widths_as_object_array(&font_info.widths[..]),
    };

    // Only add Encoding if present (symbol fonts omit it)
    if let Some(ref encoding) = font_info.encoding {
        font_dict.set("Encoding", Object::Name(encoding.as_bytes().to_vec()));
    }
    let font_bbox = bbox_as_object_array(&font_descriptor.font_bbox[..])?;
    let mut descriptor_dict = dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => font_descriptor.font_name,
        "Flags" => font_descriptor.flags,
        "FontBBox" => font_bbox,
        "ItalicAngle" => font_descriptor.italic_angle,
        "Ascent" => font_descriptor.ascent,
        "Descent" => font_descriptor.descent,
        "CapHeight" => font_descriptor.cap_height,
        "StemV" => font_descriptor.stem_v,
        "Leading" => font_descriptor.leading,
        "XHeight" => font_descriptor.x_height,
    };
    let font_file_dict = dictionary! {
        "Length1" => font_data.len() as i64,
    };
    let mut font_file = Stream::new(font_file_dict, font_data.into());
    font_file.compress()?;
    let font_file_id = dest_doc.add_object(font_file);
    descriptor_dict.set(font_descriptor.font_file_key, font_file_id);
    let descriptor_id = dest_doc.add_object(descriptor_dict);
    font_dict.set(KEY_FONT_DESCRIPTOR, descriptor_id);

    let font_id = dest_doc.add_object(font_dict);
    let font_key = register_font_in_page_resources(dest_doc, page_id, font_id)?;
    Ok((font_id, font_key, font_file_id, descriptor_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- utf8_to_winansi / unicode_to_winansi tests (from watermark_tests.rs) ---

    // --- add_rect / add_line unit tests ---

    fn create_test_page(doc: &mut Document) -> ObjectId {
        let pages_id = doc
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        let resources_id = doc.add_object(dictionary! {});
        let content_id = doc.add_object(Stream::new(dictionary! {}, vec![]));
        let media_box = vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(612.0),
            Object::Real(792.0),
        ];
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => media_box,
            "Contents" => Object::Reference(content_id),
            "Resources" => Object::Reference(resources_id),
        };
        let page_id = doc.add_object(page);
        let pages = doc.get_object_mut(pages_id).unwrap().as_dict_mut().unwrap();
        let kids = pages.get_mut(b"Kids").unwrap().as_array_mut().unwrap();
        kids.push(Object::Reference(page_id));
        pages.set("Count", Object::Integer(1));
        page_id
    }

    fn create_test_doc_and_page() -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![],
            "Count" => 0,
        };
        doc.objects
            .insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let page_id = create_test_page(&mut doc);
        (doc, page_id)
    }

    fn get_all_content_bytes(doc: &Document, page_id: ObjectId) -> Vec<u8> {
        let page = doc.get_dictionary(page_id).unwrap();
        let contents = page.get(b"Contents").unwrap();
        match contents {
            Object::Array(arr) => {
                let mut result = Vec::new();
                for item in arr {
                    if let Object::Reference(id) = item {
                        let stream = doc.get_object(*id).unwrap().as_stream().unwrap();
                        result.extend_from_slice(&stream.content);
                    }
                }
                result
            }
            Object::Reference(id) => {
                doc.get_object(*id).unwrap().as_stream().unwrap().content.clone()
            }
            _ => panic!("Unexpected Contents type"),
        }
    }

    #[test]
    fn test_add_rect_basic() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawRectParams::new(10.0, 20.0, 100.0, 50.0);
        add_rect(&mut doc, page_id, &params).unwrap();
        let content = get_all_content_bytes(&doc, page_id);
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("re"), "Should contain 're' operator");
        assert!(content_str.contains("f"), "Should contain 'f' operator");
    }

    #[test]
    fn test_add_rect_with_alpha() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawRectParams::new(0.0, 0.0, 50.0, 50.0)
            .color(crate::types::PdfColor::rgba(1.0, 0.0, 0.0, 0.5));
        add_rect(&mut doc, page_id, &params).unwrap();
        let content = get_all_content_bytes(&doc, page_id);
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("gs"), "Should contain 'gs' operator for alpha");
    }

    #[test]
    fn test_add_rect_layer_under() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawRectParams::new(0.0, 0.0, 50.0, 50.0)
            .layer_over(false);
        add_rect(&mut doc, page_id, &params).unwrap();
        // In layer_under mode, rect content is prepended
        let page = doc.get_dictionary(page_id).unwrap();
        let contents = page.get(b"Contents").unwrap().as_array().unwrap();
        assert!(contents.len() >= 2, "Should have at least 2 content streams");
    }

    #[test]
    fn test_add_line_basic() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawLineParams::new(0.0, 0.0, 100.0, 200.0);
        add_line(&mut doc, page_id, &params).unwrap();
        let content = get_all_content_bytes(&doc, page_id);
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("m"), "Should contain 'm' (moveto)");
        assert!(content_str.contains("l"), "Should contain 'l' (lineto)");
        assert!(content_str.contains("S"), "Should contain 'S' (stroke)");
    }

    #[test]
    fn test_add_line_with_alpha() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawLineParams::new(0.0, 0.0, 100.0, 100.0)
            .color(crate::types::PdfColor::rgba(0.0, 0.0, 1.0, 0.3));
        add_line(&mut doc, page_id, &params).unwrap();
        let content = get_all_content_bytes(&doc, page_id);
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("gs"), "Should contain 'gs' operator for alpha");
    }

    #[test]
    fn test_add_line_custom_width() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawLineParams::new(0.0, 0.0, 100.0, 100.0)
            .line_width(3.0);
        add_line(&mut doc, page_id, &params).unwrap();
        let content = get_all_content_bytes(&doc, page_id);
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("w"), "Should contain 'w' (line width)");
    }

    #[test]
    fn test_add_line_layer_under() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let params = crate::types::DrawLineParams::new(0.0, 0.0, 100.0, 100.0)
            .layer_over(false);
        add_line(&mut doc, page_id, &params).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        let contents = page.get(b"Contents").unwrap().as_array().unwrap();
        assert!(contents.len() >= 2, "Should have at least 2 content streams");
    }

    // --- utf8_to_winansi / unicode_to_winansi tests ---

    #[test]
    fn test_winansi_ascii_passthrough() {
        assert_eq!(utf8_to_winansi("Hello"), b"Hello".to_vec());
        assert_eq!(utf8_to_winansi("ABC123"), b"ABC123".to_vec());
        assert_eq!(utf8_to_winansi("!@#$%"), b"!@#$%".to_vec());
    }

    #[test]
    fn test_winansi_latin1_supplement() {
        assert_eq!(unicode_to_winansi('\u{00E9}'), 0xE9);
        assert_eq!(unicode_to_winansi('\u{00F1}'), 0xF1);
        assert_eq!(unicode_to_winansi('\u{00FC}'), 0xFC);
        assert_eq!(unicode_to_winansi('\u{00A9}'), 0xA9);
        assert_eq!(unicode_to_winansi('\u{00AE}'), 0xAE);
        assert_eq!(unicode_to_winansi('\u{00B0}'), 0xB0);
    }

    #[test]
    fn test_winansi_special_chars() {
        assert_eq!(unicode_to_winansi('\u{20AC}'), 0x80);
        assert_eq!(unicode_to_winansi('\u{2122}'), 0x99);
        assert_eq!(unicode_to_winansi('\u{2022}'), 0x95);
        assert_eq!(unicode_to_winansi('\u{2013}'), 0x96);
        assert_eq!(unicode_to_winansi('\u{2014}'), 0x97);
        assert_eq!(unicode_to_winansi('\u{201C}'), 0x93);
        assert_eq!(unicode_to_winansi('\u{201D}'), 0x94);
        assert_eq!(unicode_to_winansi('\u{2018}'), 0x91);
        assert_eq!(unicode_to_winansi('\u{2019}'), 0x92);
        assert_eq!(unicode_to_winansi('\u{2026}'), 0x85);
    }

    #[test]
    fn test_winansi_unmappable_fallback() {
        assert_eq!(unicode_to_winansi('\u{4E2D}'), b'?');
        assert_eq!(unicode_to_winansi('\u{65E5}'), b'?');
        assert_eq!(unicode_to_winansi('\u{03B1}'), b'?');
        assert_eq!(unicode_to_winansi('\u{2192}'), b'?');
        assert_eq!(unicode_to_winansi('\u{1F600}'), b'?');
    }

    #[test]
    fn test_winansi_cafe_example() {
        let encoded = utf8_to_winansi("Caf\u{00E9}");
        assert_eq!(encoded, vec![b'C', b'a', b'f', 0xE9]);
    }

    #[test]
    fn test_winansi_mixed_text() {
        let encoded = utf8_to_winansi("Price: \u{20AC}50");
        assert_eq!(
            encoded,
            vec![b'P', b'r', b'i', b'c', b'e', b':', b' ', 0x80, b'5', b'0']
        );
    }

    #[test]
    fn test_winansi_empty_string() {
        assert_eq!(utf8_to_winansi(""), Vec::<u8>::new());
    }

    #[test]
    fn test_winansi_copyright_notice() {
        let encoded = utf8_to_winansi("\u{00A9} 2024 Company\u{2122}");
        assert_eq!(encoded[0], 0xA9);
        assert_eq!(encoded[encoded.len() - 1], 0x99);
    }

    // --- Tests from watermark_edge_tests.rs ---

    #[test]
    fn test_winansi_ascii_range() {
        for ch in 0x00u8..=0x7F {
            let result = unicode_to_winansi(ch as char);
            assert_eq!(result, ch, "ASCII char {ch} should map directly");
        }
    }

    #[test]
    fn test_winansi_latin1_supplement_full() {
        for cp in 0xA0u32..=0xFF {
            let ch = char::from_u32(cp).unwrap();
            let result = unicode_to_winansi(ch);
            assert_eq!(result, cp as u8, "Latin-1 char U+{cp:04X} should map to {cp}");
        }
    }

    #[test]
    fn test_winansi_special_mappings() {
        assert_eq!(unicode_to_winansi('\u{20AC}'), 0x80);
        assert_eq!(unicode_to_winansi('\u{2013}'), 0x96);
        assert_eq!(unicode_to_winansi('\u{2014}'), 0x97);
        assert_eq!(unicode_to_winansi('\u{201C}'), 0x93);
        assert_eq!(unicode_to_winansi('\u{201D}'), 0x94);
        assert_eq!(unicode_to_winansi('\u{2022}'), 0x95);
        assert_eq!(unicode_to_winansi('\u{2122}'), 0x99);
        assert_eq!(unicode_to_winansi('\u{0152}'), 0x8C);
        assert_eq!(unicode_to_winansi('\u{0153}'), 0x9C);
    }

    #[test]
    fn test_winansi_unmappable_chars() {
        assert_eq!(unicode_to_winansi('\u{4E2D}'), b'?');
        assert_eq!(unicode_to_winansi('\u{1F600}'), b'?');
        assert_eq!(unicode_to_winansi('\u{0627}'), b'?');
    }

    #[test]
    fn test_utf8_to_winansi_mixed_string() {
        let result = utf8_to_winansi("Hello\u{20AC}World");
        assert_eq!(result, b"Hello\x80World");
    }

    #[test]
    fn test_utf8_to_winansi_empty() {
        let result = utf8_to_winansi("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_utf8_to_winansi_all_unmappable() {
        let result = utf8_to_winansi("\u{4E2D}\u{6587}");
        assert_eq!(result, b"??");
    }

    // --- EmbeddedFontCache unit tests ---

    #[test]
    fn test_embedded_font_cache_default() {
        let cache = EmbeddedFontCache::default();
        assert!(cache.cache.is_empty());
    }

    #[test]
    fn test_embedded_font_cache_insert_and_get() {
        let mut cache = EmbeddedFontCache::new();
        let data = Arc::new(vec![1, 2, 3]);
        let font_id = (42, 0);
        cache.insert(&data, font_id, "F42".into(), (100, 0), (101, 0));

        let entry = cache.get(&data);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.font_id, (42, 0));
        assert_eq!(entry.font_key, "F42");
        assert_eq!(entry.font_stream_id, (100, 0));
        assert_eq!(entry.descriptor_id, (101, 0));
        assert!(entry.used_chars.is_empty());
    }

    #[test]
    fn test_embedded_font_cache_same_arc_hits() {
        let mut cache = EmbeddedFontCache::new();
        let data = Arc::new(vec![1, 2, 3]);
        cache.insert(&data, (10, 0), "F10".into(), (100, 0), (101, 0));

        // Clone the Arc — same pointer, should hit cache
        let data_clone = Arc::clone(&data);
        let result = cache.get(&data_clone);
        assert!(result.is_some());
    }

    #[test]
    fn test_embedded_font_cache_different_arc_misses() {
        let mut cache = EmbeddedFontCache::new();
        let data1 = Arc::new(vec![1, 2, 3]);
        cache.insert(&data1, (10, 0), "F10".into(), (100, 0), (101, 0));

        // Different Arc with identical content — different pointer, should miss
        let data2 = Arc::new(vec![1, 2, 3]);
        let result = cache.get(&data2);
        assert!(result.is_none());
    }

    #[test]
    fn test_embedded_font_cache_multiple_fonts() {
        let mut cache = EmbeddedFontCache::new();
        let font_a = Arc::new(vec![1, 2, 3]);
        let font_b = Arc::new(vec![4, 5, 6]);
        cache.insert(&font_a, (10, 0), "F10".into(), (100, 0), (101, 0));
        cache.insert(&font_b, (20, 0), "F20".into(), (200, 0), (201, 0));

        let entry_a = cache.get(&font_a).unwrap();
        assert_eq!(entry_a.font_id, (10, 0));
        assert_eq!(entry_a.font_key, "F10");

        let entry_b = cache.get(&font_b).unwrap();
        assert_eq!(entry_b.font_id, (20, 0));
        assert_eq!(entry_b.font_key, "F20");
    }

    #[test]
    fn test_embedded_font_cache_miss_on_empty() {
        let cache = EmbeddedFontCache::new();
        let data = Arc::new(vec![1, 2, 3]);
        assert!(cache.get(&data).is_none());
    }

    #[test]
    fn test_embedded_font_cache_record_chars() {
        let mut cache = EmbeddedFontCache::new();
        let data = Arc::new(vec![1, 2, 3]);
        cache.insert(&data, (10, 0), "F10".into(), (100, 0), (101, 0));

        cache.record_chars(&data, "DRAFT");
        let entry = cache.get(&data).unwrap();
        assert_eq!(entry.used_chars.len(), 5);
        assert!(entry.used_chars.contains(&'D'));
        assert!(entry.used_chars.contains(&'R'));
        assert!(entry.used_chars.contains(&'A'));
        assert!(entry.used_chars.contains(&'F'));
        assert!(entry.used_chars.contains(&'T'));

        // Recording again with overlapping chars shouldn't double them
        cache.record_chars(&data, "DATA");
        let entry = cache.get(&data).unwrap();
        assert_eq!(entry.used_chars.len(), 5); // still 5 — no new unique chars
    }
}
