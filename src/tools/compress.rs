//! Tool 14: compress an existing PDF.
//!
//! Page order and every content stream stay untouched. Two things shrink the
//! file: image XObjects are re-encoded to a strength-selected quality / DPI
//! budget (the two bulk sources in scanned documents), and objects that
//! nothing references any more — leftovers from page operations or replaced
//! images — are dropped. Text, vectors and fonts are byte-identical.
//!
//! Only *additive* losses are taken: a re-encode that does not save at least
//! 5 % of the stream keeps the original bytes, CMYK and 1-bit images stay as
//! they are, and a single image that fails to decode never fails the run.

use std::collections::{BTreeMap, BTreeSet};

use lopdf::{dictionary, Object, ObjectId, Stream};

use crate::engine::{
    pdf_io,
    raster::Matrix,
    render,
    validate,
};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel,
    image_embed::{jpeg_from_dynamic, resample, target_pixels, ScanBudget},
    split::{ensure_not_empty, ensure_pdf, single_input},
    CancelFlag, CompressLevel, Progress, Tool, ToolOptions, ToolRequest,
};

/// Streams above this size are left alone: decoding a monster image risks
/// memory for a tool whose whole point is to make files smaller.
const MAX_STREAM_BYTES: usize = 48 * 1024 * 1024;

/// Images larger than this on either axis are skipped, mirroring the renderer.
const MAX_PIXELS: u32 = 8000;

/// A re-encode must keep at least 5 % of the stream, or the original bytes
/// stay: a second lossy pass for 2 % is never worth the quality.
const MIN_GAIN: f64 = 0.95;

/// Colour semantics an image needs before it can be re-encoded as JPEG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channels {
    Gray,
    Rgb,
}

pub struct Compress;

impl Tool for Compress {
    fn id(&self) -> ToolId {
        ToolId::Compress
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        ensure_pdf(single_input(req)?)?;
        Ok(())
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let path = single_input(req)?;
        let level = match &req.options {
            ToolOptions::Compress { level } => *level,
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        // Saving a decrypted copy would silently strip the open password, so
        // encrypted inputs are refused with the escape hatch spelled out.
        if pdf_io::is_encrypted(path)? {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "加密的 PDF 压缩后会移除打开密码；请先用「加密与解密」移除密码，再压缩"
                    .to_string(),
            });
        }

        // Kept for the size comparison and the no-gain fallback below.
        let original = std::fs::read(path).map_err(|e| ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;

        let mut doc = pdf_io::open_for_edit(path, None)?;
        let total = pdf_io::page_count(&doc);
        ensure_not_empty(total, path)?;

        // Materialise inherited page attributes so the drawn-size scan finds
        // each page's image resources (in-memory only, never saved back).
        let page_ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        for page_id in &page_ids {
            pdf_io::inherit_page_attributes(&mut doc, *page_id)?;
        }

        progress.report(0.05, "分析图片".to_string());
        // Objects nothing references are dropped before the file is written, so
        // working on them first would spend a JPEG round-trip on bytes that
        // never reach the output — and it made the summary report more images
        // than the document shows.
        let stale = collect_orphans(&doc);
        let drawn = scan_drawn_sizes(&doc);
        let (replaced, skipped) =
            compress_images(&mut doc, &drawn, &stale, level, progress, cancel)?;

        progress.report(0.8, "清理未引用对象".to_string());
        check_cancel(cancel)?;
        let orphans = collect_orphans(&doc);
        doc.objects.retain(|id, _| !orphans.contains(id));

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        let compressed = pdf_io::document_bytes(&mut doc)?;
        let compressed_len = compressed.len();

        // A "compressed" file that grew would be worse than useless: fall back
        // to the original bytes and say so. Both branches keep the page count,
        // which is the invariant the assertion checks.
        //
        // The page count leads the sentence on purpose: the image count is per
        // XObject, and a page that carries two images (a merged half-page scan)
        // makes the two numbers differ — reporting only "13 张" read like "13
        // 页" and looked like a wrong page count.
        let (payload, summary) = if compressed_len < original.len() {
            let gain = 100.0 - compressed_len as f64 / original.len() as f64 * 100.0;
            // A page can carry two images (a merged half-page scan), so the
            // image count legitimately exceeds the page count; say so instead
            // of letting "13 张" read as "13 页".
            let mixed = if replaced > total as usize {
                "（有页面含多张图片）"
            } else {
                ""
            };
            (
                compressed,
                format!(
                    "已压缩：{} → {}（−{gain:.0}%）；{total} 页，重编码图片 {replaced} 张{mixed}、跳过 {skipped} 张，清理未引用对象 {} 个",
                    fmt_size(original.len()),
                    fmt_size(compressed_len),
                    orphans.len(),
                ),
            )
        } else {
            (
                original,
                format!(
                    "未能进一步减小体积（原文件已充分优化），已按原样保存；{total} 页，重编码图片 {replaced} 张，清理未引用对象 {} 个",
                    orphans.len(),
                ),
            )
        };
        pdf_io::write_file_atomic(&out, &payload, req.conflict)?;
        validate::assert_page_count(&out, total, None)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(summary, vec![out]))
    }
}

/// Re-encode every compressible image XObject; returns (replaced, skipped).
///
/// `stale` holds the objects the orphan sweep is about to delete: an image
/// nothing references is not part of the document, so decoding and re-encoding
/// it would waste a round-trip and inflate the reported image count.
fn compress_images(
    doc: &mut lopdf::Document,
    drawn: &BTreeMap<ObjectId, (f32, f32)>,
    stale: &BTreeSet<ObjectId>,
    level: CompressLevel,
    progress: &Progress,
    cancel: &CancelFlag,
) -> Result<(usize, usize), ToolError> {
    let images = compressible_images(doc, stale);

    let mut replaced = 0usize;
    let mut skipped = 0usize;
    let count = images.len();
    for (index, id) in images.iter().enumerate() {
        check_cancel(cancel)?;
        progress.report(
            0.05 + 0.7 * index as f32 / count.max(1) as f32,
            format!("压缩图片 {}/{}", index + 1, count),
        );

        // Clone the stream so the decoder borrows freely while `doc` stays
        // mutable for the replacement.
        let Some(stream) = doc
            .objects
            .get(id)
            .and_then(|object| object.as_stream().ok().cloned())
        else {
            continue;
        };
        match recompress_image(doc, *id, &stream, drawn, level) {
            Ok(true) => replaced += 1,
            _ => skipped += 1,
        }
    }
    Ok((replaced, skipped))
}

/// Re-encode one image XObject in place. `Ok(true)` = replaced.
///
/// Every failure mode is a skip, not an error: one unreadable image must not
/// take the whole run down when keeping the original bytes is perfectly fine.
fn recompress_image(
    doc: &mut lopdf::Document,
    id: ObjectId,
    stream: &Stream,
    drawn: &BTreeMap<ObjectId, (f32, f32)>,
    level: CompressLevel,
) -> Result<bool, ()> {
    let dict = &stream.dict;
    let width = dict.get(b"Width").ok().and_then(as_u32).ok_or(())?;
    let height = dict.get(b"Height").ok().and_then(as_u32).ok_or(())?;
    if width == 0 || height == 0 || width > MAX_PIXELS || height > MAX_PIXELS {
        return Ok(false);
    }
    if stream.content.is_empty() || stream.content.len() > MAX_STREAM_BYTES {
        return Ok(false);
    }
    // Non-default /Decode flips channels and /Mask carries its own semantics;
    // both would silently change what the image looks like.
    if dict.get(b"Decode").is_ok() || dict.get(b"Mask").is_ok() {
        return Ok(false);
    }
    if dict
        .get(b"ImageMask")
        .ok()
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false)
    {
        return Ok(false);
    }
    // Only plain 8-bit samples: bilevel scans are already tiny, and 16-bit
    // re-encodes would halve quality for nothing.
    if dict.get(b"BitsPerComponent").ok().and_then(as_u32) != Some(8) {
        return Ok(false);
    }
    let channels = match colorspace_channels(doc, dict) {
        Some(channels) => channels,
        None => return Ok(false),
    };

    let Some(rgba) = render::decode_image_pixels(doc, stream, [1.0, 1.0, 1.0, 1.0]) else {
        return Ok(false);
    };
    if rgba.width == 0 || rgba.height == 0 {
        return Ok(false);
    }

    let drawn_size = drawn.get(&id).copied().unwrap_or((0.0, 0.0));
    let (tw, th) = target_pixels(rgba.width, rgba.height, drawn_size, level.max_dpi());
    if tw == 0 || th == 0 {
        return Ok(false);
    }

    let image = match channels {
        Channels::Gray => image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(
                rgba.width,
                rgba.height,
                rgba.data.chunks_exact(4).map(|p| p[0]).collect(),
            )
            .ok_or(())?,
        ),
        Channels::Rgb => image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(
                rgba.width,
                rgba.height,
                rgba.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
            )
            .ok_or(())?,
        ),
    };
    let budget = ScanBudget::new(level.max_dpi(), level.quality());
    let encoded = jpeg_from_dynamic(image, (tw, th), budget).map_err(|_| ())?;

    // Keep the original bytes when the second lossy pass buys (almost) nothing.
    if (encoded.data.len() as f64) >= stream.content.len() as f64 * MIN_GAIN {
        return Ok(false);
    }

    let mut new_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => tw as i64,
        "Height" => th as i64,
        "ColorSpace" => match channels {
            Channels::Gray => "DeviceGray",
            Channels::Rgb => "DeviceRGB",
        },
        "BitsPerComponent" => 8i64,
        "Filter" => "DCTDecode",
    };
    if let Ok(smask) = dict.get(b"SMask") {
        new_dict.set("SMask", smask.clone());
        // The mask tracks the image it masks; a decode failure here only costs
        // a few bytes — PDF viewers scale masks to the image rectangle.
        if let Ok(smask_id) = dict.get(b"SMask").and_then(|o| o.as_reference()) {
            let _ = resample_smask(doc, smask_id, drawn_size, level);
        }
    }

    let jpeg = Stream::new(new_dict, encoded.data).with_compression(false);
    doc.objects.insert(id, Object::Stream(jpeg));
    Ok(true)
}

/// Re-encode one soft mask as a plain 8-bit gray Flate stream.
fn resample_smask(
    doc: &mut lopdf::Document,
    smask_id: ObjectId,
    drawn_size: (f32, f32),
    level: CompressLevel,
) -> Result<(), ()> {
    let Some(stream) = doc
        .objects
        .get(&smask_id)
        .and_then(|object| object.as_stream().ok().cloned())
    else {
        return Err(());
    };
    let dict = &stream.dict;
    let width = dict.get(b"Width").ok().and_then(as_u32).ok_or(())?;
    let height = dict.get(b"Height").ok().and_then(as_u32).ok_or(())?;
    if width == 0 || height == 0 || width > MAX_PIXELS || height > MAX_PIXELS {
        return Err(());
    }
    if stream.content.len() > MAX_STREAM_BYTES {
        return Err(());
    }
    if dict.get(b"Decode").is_ok()
        || dict
            .get(b"ImageMask")
            .ok()
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false)
    {
        return Err(());
    }
    // Masks must be 8-bit gray; anything else stays as it is.
    if dict.get(b"BitsPerComponent").ok().and_then(as_u32) != Some(8) {
        return Err(());
    }

    let Some(rgba) = render::decode_image_pixels(doc, &stream, [1.0, 1.0, 1.0, 1.0]) else {
        return Err(());
    };
    let pixels: Vec<u8> = rgba.data.chunks_exact(4).map(|p| p[0]).collect();
    let (tw, th) = target_pixels(rgba.width, rgba.height, drawn_size, level.max_dpi());
    if tw == 0 || th == 0 || tw == rgba.width && th == rgba.height {
        // Nothing to resample: the existing mask stream is kept untouched.
        return Ok(());
    }
    let resampled =
        resample(pixels, rgba.width, rgba.height, tw, th, fast_image_resize::PixelType::U8)
            .map_err(|_| ())?;
    // Compressed here, explicitly: lopdf's `Stream::compress` refuses a
    // dictionary that already declares /Filter, which would silently store raw
    // pixels behind a FlateDecode claim.
    use std::io::Write as _;
    let compressed = {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&resampled).map_err(|_| ())?;
        encoder.finish().map_err(|_| ())?
    };

    let new_stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => tw as i64,
            "Height" => th as i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8i64,
            "Filter" => "FlateDecode",
        },
        compressed,
    );
    doc.objects.insert(smask_id, Object::Stream(new_stream));
    Ok(())
}

/// Which JPEG channel set an image's colour space maps onto, if any.
///
/// CMYK (plain or ICC-based) is deliberately out: JPEG round-trips of CMYK
/// scans depend on the Adobe inversion convention, and getting it wrong would
/// wreck the very content the tool promises to keep.
/// Byte-slice name comparison: PDF names are raw bytes and `as_name` yields
/// `&[u8]`, so every match goes through here.
fn named(name: &[u8], want: &str) -> bool {
    name == want.as_bytes()
}

fn colorspace_channels(doc: &lopdf::Document, dict: &lopdf::Dictionary) -> Option<Channels> {
    match dict.get(b"ColorSpace").ok()? {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" => Some(Channels::Gray),
            b"DeviceRGB" => Some(Channels::Rgb),
            _ => None,
        },
        Object::Array(items) => {
            let family = items.first()?.as_name().ok()?;
            if named(family, "ICCBased") {
                let id = items.get(1)?.as_reference().ok()?;
                let stream = doc.get_object(id).ok()?.as_stream().ok()?;
                match stream.dict.get(b"N").ok()?.as_i64().ok()? {
                    1 => Some(Channels::Gray),
                    3 => Some(Channels::Rgb),
                    _ => None,
                }
            } else if named(family, "Indexed") {
                // The renderer expands the palette; only the base channel
                // count decides gray vs RGB.
                match items.get(1)? {
                    Object::Name(base) => match base.as_slice() {
                        b"DeviceGray" => Some(Channels::Gray),
                        b"DeviceRGB" => Some(Channels::Rgb),
                        _ => None,
                    },
                    Object::Array(base) => {
                        // e.g. [/Indexed [/ICCBased …] …]
                        let inner = base.first()?.as_name().ok()?;
                        if !named(inner, "ICCBased") {
                            return None;
                        }
                        let id = base.get(1)?.as_reference().ok()?;
                        let stream = doc.get_object(id).ok()?.as_stream().ok()?;
                        match stream.dict.get(b"N").ok()?.as_i64().ok()? {
                            1 => Some(Channels::Gray),
                            3 => Some(Channels::Rgb),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Display size of every image drawn directly on a page, in points.
///
/// The effective DPI of an image decides whether resampling buys anything, and
/// that needs the rectangle it is painted into. Only top-level page content is
/// scanned — images inside form XObjects fall back to quality-only re-encoding,
/// which is the conservative direction.
fn scan_drawn_sizes(doc: &lopdf::Document) -> BTreeMap<ObjectId, (f32, f32)> {
    let mut sizes: BTreeMap<ObjectId, (f32, f32)> = BTreeMap::new();
    for page_id in doc.get_pages().values() {
        let Some(xobjects) = page_xobjects(doc, *page_id) else {
            continue;
        };
        let operations = page_content_operations(doc, *page_id);
        let operations = match operations {
            Some(operations) => operations,
            None => continue,
        };

        let mut stack: Vec<Matrix> = vec![Matrix::IDENTITY];
        for operation in &operations {
            match operation.operator.as_str() {
                "q" => stack.push(*stack.last().unwrap_or(&Matrix::IDENTITY)),
                "Q" => {
                    stack.pop();
                    if stack.is_empty() {
                        stack.push(Matrix::IDENTITY);
                    }
                }
                "cm" if operation.operands.len() == 6 => {
                    let numbers: Vec<f32> = operation
                        .operands
                        .iter()
                        .filter_map(|o| {
                            o.as_float()
                                .ok()
                                .or_else(|| o.as_i64().ok().map(|v| v as f32))
                        })
                        .collect();
                    if numbers.len() == 6 {
                        let m = Matrix {
                            a: numbers[0],
                            b: numbers[1],
                            c: numbers[2],
                            d: numbers[3],
                            e: numbers[4],
                            f: numbers[5],
                        };
                        if let Some(current) = stack.last_mut() {
                            *current = m.mul(current);
                        }
                    }
                }
                "Do" if operation.operands.len() == 1 => {
                    let Some(name) = operation.operands[0].as_name().ok() else {
                        continue;
                    };
                    let Some(id) = xobjects.get(name) else {
                        continue;
                    };
                    if let Some(stream) = doc.get_object(*id).ok().and_then(|o| o.as_stream().ok())
                    {
                        if !is_image_object(&stream.dict) {
                            continue;
                        }
                    } else {
                        continue;
                    }
                    let ctm = *stack.last().unwrap_or(&Matrix::IDENTITY);
                    let drawn = ((ctm.a * ctm.a + ctm.b * ctm.b).sqrt(), (ctm.c * ctm.c + ctm.d * ctm.d).sqrt());
                    if drawn.0 > 0.0 && drawn.1 > 0.0 {
                        // An image painted several times gets the most
                        // generous rectangle: quality is decided by the
                        // largest place it is shown.
                        let entry = sizes.entry(*id).or_insert((0.0, 0.0));
                        entry.0 = entry.0.max(drawn.0);
                        entry.1 = entry.1.max(drawn.1);
                    }
                }
                _ => {}
            }
        }
    }
    sizes
}

/// The `/XObject` dictionary of one page (inheritance is materialised by the
/// caller), mapping resource names to object ids.
fn page_xobjects(
    doc: &lopdf::Document,
    page_id: ObjectId,
) -> Option<BTreeMap<Vec<u8>, ObjectId>> {
    let resources = doc.get_object(page_id).ok()?.as_dict().ok()?.get(b"Resources").ok()?;
    let resources = match resources {
        Object::Reference(id) => doc.get_object(*id).ok()?.as_dict().ok()?.clone(),
        Object::Dictionary(dict) => dict.clone(),
        _ => return None,
    };
    let xobjects = resources.get(b"XObject").ok()?.as_dict().ok()?;
    let mut map = BTreeMap::new();
    for (name, value) in xobjects.iter() {
        if let Ok(id) = value.as_reference() {
            map.insert(name.clone(), id);
        }
    }
    Some(map)
}

/// Image XObjects worth re-encoding: still reachable, and not a mask.
///
/// Masks are optimised alongside the image that references them (a JPEG
/// round-trip would blur what is usually a bilevel alpha shape). `stale` is the
/// orphan set — an image nothing references is about to be deleted, so it must
/// not be decoded, re-encoded or counted.
fn compressible_images(doc: &lopdf::Document, stale: &BTreeSet<ObjectId>) -> Vec<ObjectId> {
    let masks = mask_ids(doc);
    doc.objects
        .iter()
        .filter(|(id, object)| {
            !stale.contains(id)
                && !masks.contains(id)
                && object
                    .as_stream()
                    .is_ok_and(|stream| is_image_object(&stream.dict))
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Objects an image uses as its soft mask / mask.
fn mask_ids(doc: &lopdf::Document) -> BTreeSet<ObjectId> {
    doc.objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .filter(|stream| is_image_object(&stream.dict))
        .filter_map(|stream| {
            for key in [&b"SMask"[..], b"Mask"] {
                if let Ok(Object::Reference(id)) = stream.dict.get(key) {
                    return Some(*id);
                }
            }
            None
        })
        .collect()
}

/// The page's parsed content operations: `/Contents` may be one reference or an
/// array of them, each stream optionally filtered.
fn page_content_operations(
    doc: &lopdf::Document,
    page_id: ObjectId,
) -> Option<Vec<lopdf::content::Operation>> {
    let page = doc.get_object(page_id).ok()?.as_dict().ok()?;
    let contents = page.get(b"Contents").ok()?.clone();
    let mut ids: Vec<ObjectId> = Vec::new();
    match contents {
        Object::Reference(id) => ids.push(id),
        Object::Array(items) => {
            for item in items {
                if let Ok(id) = item.as_reference() {
                    ids.push(id);
                }
            }
        }
        _ => return None,
    }

    let mut operations = Vec::new();
    for id in ids {
        let Ok(stream) = doc.get_object(id).and_then(|object| object.as_stream()) else {
            continue;
        };
        // The renderer's rule applies here too: `decompressed_content` refuses
        // an unfiltered stream, so those are taken verbatim.
        let data = if stream.dict.get(b"Filter").is_ok() {
            match stream.decompressed_content() {
                Ok(data) => data,
                Err(_) => continue,
            }
        } else {
            stream.content.clone()
        };
        let parsed = lopdf::content::Content::decode(&data).ok()?;
        operations.extend(parsed.operations);
    }
    Some(operations)
}

fn is_image_object(dict: &lopdf::Dictionary) -> bool {
    dict.get(b"Subtype")
        .is_ok_and(|o| o.as_name().is_ok_and(|n| named(n, "Image")))
}

fn as_u32(object: &Object) -> Option<u32> {
    object
        .as_i64()
        .ok()
        .filter(|value| *value >= 0 && *value <= u32::MAX as i64)
        .map(|value| value as u32)
}

/// Objects nothing reachable from the trailer references.
///
/// Replaced image streams and page-operation leftovers are the usual bulk.
/// Reachability follows every reference in dictionaries and arrays; stream
/// payloads carry no references, so their dictionaries are the frontier.
///
/// The traversal is iterative: a bookmark chain (`/Next` after `/Next`) or a
/// linear page tree makes the graph as deep as the file is long, and the worker
/// thread runs on Rust's default stack. Recursing one frame per hop would turn
/// a crafted file into a stack overflow, which aborts the process before any
/// `ToolError` can be reported.
fn collect_orphans(doc: &lopdf::Document) -> BTreeSet<ObjectId> {
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    let mut stack: Vec<&Object> = Vec::new();
    for key in [&b"Root"[..], b"Info"] {
        if let Ok(object) = doc.trailer.get(key) {
            stack.push(object);
        }
    }
    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(id) => {
                if seen.insert(*id) {
                    if let Some(next) = doc.objects.get(id) {
                        stack.push(next);
                    }
                }
            }
            Object::Array(items) => stack.extend(items.iter()),
            Object::Dictionary(dict) => stack.extend(dict.iter().map(|(_, value)| value)),
            Object::Stream(stream) => stack.extend(stream.dict.iter().map(|(_, value)| value)),
            _ => {}
        }
    }
    doc.objects
        .keys()
        .filter(|id| !seen.contains(id))
        .copied()
        .collect()
}

/// Human-readable size for the summary line.
fn fmt_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1_024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drawn-size scan must find the image rectangle through `cm` + `Do`.
    #[test]
    fn scan_finds_drawn_rectangles() {
        use lopdf::{dictionary, Object, Stream};

        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 2000i64,
                "Height" => 2000i64,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8i64,
            },
            vec![0u8; 16],
        ));
        let content_id = doc.add_object(Stream::new(
            Default::default(),
            b"q 400 0 0 400 0 0 cm /Im1 Do Q".to_vec(),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 400.into(), 400.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image_id } },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => 1i64,
                "Kids" => vec![page_id.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        for page_id in doc.get_pages().values().copied().collect::<Vec<_>>() {
            pdf_io::inherit_page_attributes(&mut doc, page_id).unwrap();
        }
        let drawn = scan_drawn_sizes(&doc);
        assert_eq!(
            drawn.get(&image_id),
            Some(&(400.0, 400.0)),
            "the scan must find the drawn rectangle, got {drawn:?}"
        );
    }

    /// Leftovers must not be decoded, re-encoded or counted: an unreferenced
    /// image is deleted by the orphan sweep, and a mask is optimised with the
    /// image it masks. Either one showing up here made the summary report more
    /// images than the document contains.
    #[test]
    fn compressible_images_skip_orphans_and_masks() {
        use lopdf::{dictionary, ObjectId, Stream};

        let mut doc = lopdf::Document::with_version("1.5");
        let image = |doc: &mut lopdf::Document, width: i64| {
            doc.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => width,
                    "Height" => width,
                    "ColorSpace" => "DeviceRGB",
                    "BitsPerComponent" => 8i64,
                },
                vec![0u8; 16],
            ))
        };
        let used = image(&mut doc, 100);
        let orphan = image(&mut doc, 200);
        let mask = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 100i64,
                "Height" => 100i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8i64,
            },
            vec![0u8; 8],
        ));
        if let Ok(lopdf::Object::Stream(stream)) = doc.get_object_mut(used) {
            stream.dict.set("SMask", lopdf::Object::Reference(mask));
        }

        let stale: BTreeSet<ObjectId> = BTreeSet::from([orphan]);
        let images = compressible_images(&doc, &stale);
        assert_eq!(
            images,
            vec![used],
            "只有被引用的非蒙版图片参与重编码：{images:?}"
        );
    }
}
