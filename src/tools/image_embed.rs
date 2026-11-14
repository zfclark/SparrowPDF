//! Shared image-embedding plumbing for the two image→PDF tools.
//!
//! The pipeline has two halves, split on purpose:
//!
//! * [`prepare_image`] does everything expensive — decode, downscale, JPEG
//!   encode — and returns layout-independent bytes plus the placement matrix.
//!   Because it needs nothing but the slot the image is drawn into, the batch
//!   tool runs it for many images at once on a small thread pool.
//! * [`add_page`] only writes PDF objects, so it stays cheap and serial.
//!
//! Resolution policy:
//!
//! * JPEG inputs within the resolution budget keep their original bytes
//!   (`DCTDecode`, no re-encode).
//! * A source carrying more pixels than the page can show (a 600 DPI flatbed
//!   scan, a 12 MP phone photo) is downsampled to the budget and re-encoded —
//!   the dropped pixels could never reach paper.
//! * Opaque PNG inputs are re-encoded as JPEG: Flate over raw pixels is an
//!   order of magnitude larger for a photo-like scan. PNGs with real
//!   transparency keep the Flate + `SMask` path.
//! * EXIF orientation is honoured by adjusting the placement matrix rather than
//!   by rotating pixels, which is what keeps the JPEG byte stream intact.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageFormat};
use lopdf::{dictionary, Document, Object, ObjectId};

use crate::model::ToolError;
use crate::tools::{Orientation, PageSize};

/// Margin around each image, in points.
pub(crate) const PAGE_MARGIN: f32 = 0.0;

/// Resolution and quality a scan may be stored at.
///
/// `max_dpi` caps the effective resolution the image occupies **after** it is
/// drawn on the page (a 4958 px scan in an A4 box is 600 DPI; at 300 DPI it
/// only needs 2480 px). `quality` is the JPEG quality of the re-encode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScanBudget {
    pub max_dpi: f32,
    pub quality: u8,
}

/// Ladder floors: below these a further pass costs detail without buying
/// meaningful size, so the size guard stops instead of grinding.
const MIN_DPI: f32 = 96.0;
const MIN_QUALITY: u8 = 45;

impl ScanBudget {
    pub(crate) const fn new(max_dpi: f32, quality: u8) -> Self {
        Self { max_dpi, quality }
    }

    /// One step tighter, or `None` once the floor is reached.
    pub(crate) fn tightened(self) -> Option<Self> {
        let max_dpi = (self.max_dpi * 0.82).max(MIN_DPI);
        let quality = self.quality.saturating_sub(6).max(MIN_QUALITY);
        if (max_dpi - self.max_dpi).abs() < 1.0 && quality == self.quality {
            return None;
        }
        Some(Self { max_dpi, quality })
    }
}

/// Where an image sits on its page.
///
/// Built by the caller from the layout policy, before any pixel is touched, so
/// preparation never has to know about templates, margins or merge specs.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Sheet {
    /// Page box and image slot are fixed by the layout.
    Fixed {
        /// MediaBox of the page, in points.
        page: (f32, f32),
        /// Box the image is fitted into.
        fit: (f32, f32),
        /// Point the image is centred on.
        center: (f32, f32),
        /// Orientation the image should follow (page box, or slot).
        landscape: bool,
        /// Turn an image that conflicts with `landscape` by 90°.
        auto_rotate: bool,
    },
    /// The page follows the image: one image on a sheet in its own direction.
    Follow { page_size: PageSize, margin: f32 },
}

/// An image whose pixels have already been decoded, scaled and encoded.
pub(crate) struct PreparedImage {
    object: ImageObject,
    /// Placement matrix, EXIF orientation and the optional 90° turn folded in.
    matrix: [f32; 6],
    /// MediaBox the image needs; a `Sheet::Follow` page takes this one.
    pub page_box: (f32, f32),
    /// Whether the payload is the caller's original JPEG (no re-encode).
    pub jpeg_passthrough: bool,
}

/// XObject payload of a prepared image.
enum ImageObject {
    /// Complete JPEG stream for a `/DCTDecode` XObject.
    Jpeg(EncodedJpeg),
    /// Samples for a `/FlateDecode` XObject, with an optional `/SMask`.
    Raster {
        pixels: Vec<u8>,
        alpha: Option<Vec<u8>>,
        width: u32,
        height: u32,
        /// 1 = Gray, 3 = RGB.
        components: u8,
    },
}

/// A JPEG ready for a `DCTDecode` stream.
struct EncodedJpeg {
    data: Vec<u8>,
    width: u32,
    height: u32,
    /// 1 = Gray, 3 = RGB, 4 = CMYK.
    components: u8,
}

/// Decoded input, kept in the shape that avoids a needless re-encode.
enum Source {
    Jpeg {
        data: Vec<u8>,
        width: u32,
        height: u32,
        components: u8,
    },
    Raster(DynamicImage),
}

impl Source {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Source::Jpeg { width, height, .. } => (*width, *height),
            Source::Raster(image) => (image.width(), image.height()),
        }
    }
}

/// Decode, downscale and encode one image for the slot it will occupy.
///
/// This is the expensive half of the pipeline and the unit of work the batch
/// tool parallelises: it touches no `Document`, so any number of these can run
/// at once.
pub(crate) fn prepare_image(
    path: &Path,
    sheet: Sheet,
    budget: Option<ScanBudget>,
) -> Result<PreparedImage, ToolError> {
    let source = load_source(path)?;
    let (px_w, px_h) = source.dimensions();
    if px_w == 0 || px_h == 0 {
        return Err(ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "图片尺寸为 0".to_string(),
        });
    }

    // Orientations 5..=8 display the image rotated by 90 degrees, so the
    // effective width/height are swapped.
    let orientation = match source {
        Source::Jpeg { .. } => read_exif_orientation(path),
        Source::Raster(_) => 1,
    };
    let (eff_w, eff_h) = if matches!(orientation, 5..=8) {
        (px_h as f32, px_w as f32)
    } else {
        (px_w as f32, px_h as f32)
    };
    let image_is_landscape = eff_w > eff_h;

    let (page, fit, center, rotate) = match sheet {
        Sheet::Fixed {
            page,
            fit,
            center,
            landscape,
            auto_rotate,
        } => (
            page,
            fit,
            center,
            auto_rotate && image_is_landscape != landscape,
        ),
        Sheet::Follow { page_size, margin } => {
            let page = page_size.page(if image_is_landscape {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            });
            // A page that follows the image never needs a 90° turn: the sheet
            // is already in the image's own direction.
            (
                page,
                (page.0 - margin * 2.0, page.1 - margin * 2.0),
                (page.0 / 2.0, page.1 / 2.0),
                false,
            )
        }
    };

    let (fit_w, fit_h) = if rotate {
        (eff_h, eff_w)
    } else {
        (eff_w, eff_h)
    };
    let scale = (fit.0 / fit_w).min(fit.1 / fit_h);
    let draw_w = fit_w * scale;
    let draw_h = fit_h * scale;
    let x0 = center.0 - draw_w / 2.0;
    let y0 = center.1 - draw_h / 2.0;

    let code = if rotate {
        rotate_cw_code(orientation)
    } else {
        orientation
    };
    let matrix = placement_matrix(code, x0, y0, draw_w, draw_h);

    // Effective resolution is measured on the *displayed* geometry, so the
    // pixel count has to be taken in the same frame as `drawn` (`fit_*`), and
    // mapped back to the file's own orientation afterwards — the matrix does
    // the turning, the raster stays as stored.
    //
    // The mapping depends on both turns: EXIF rotation and the 90° slot
    // rotation compose, so only one of them swaps the axes.
    let exif_swapped = matches!(orientation, 5..=8);
    let target = match budget {
        Some(budget) => {
            let (tw, th) =
                target_pixels(fit_w as u32, fit_h as u32, (draw_w, draw_h), budget.max_dpi);
            if exif_swapped != rotate {
                (th, tw)
            } else {
                (tw, th)
            }
        }
        None => (px_w, px_h),
    };

    let is_jpeg = matches!(source, Source::Jpeg { .. });
    let object = build_object(source, target, budget)?;

    Ok(PreparedImage {
        object,
        matrix,
        page_box: page,
        jpeg_passthrough: is_jpeg,
    })
}

/// Turn the decoded source into an XObject payload.
///
/// `target` is the pixel size to store, already derived from the *displayed*
/// resolution budget; the source's own orientation is preserved.
fn build_object(
    source: Source,
    target: (u32, u32),
    budget: Option<ScanBudget>,
) -> Result<ImageObject, ToolError> {
    match source {
        Source::Jpeg {
            data,
            width,
            height,
            components,
        } => {
            // Re-encoding is only safe for the colour models the `image` crate
            // decodes reliably (Gray / RGB). CMYK scans stay byte-for-byte:
            // a wrong conversion would be worse than a large file.
            let budget = budget.filter(|_| matches!(components, 1 | 3));
            if let Some(budget) = budget {
                // Equal size means the source already fits: keep it verbatim
                // rather than paying a second JPEG generation for nothing.
                if target != (width, height) {
                    let decoded =
                        image::load_from_memory_with_format(&data, ImageFormat::Jpeg).ok();
                    if let Some(image) = decoded {
                        if let Ok(jpeg) = jpeg_from_dynamic(image, target, budget) {
                            return Ok(ImageObject::Jpeg(jpeg));
                        }
                    }
                }
            }
            Ok(ImageObject::Jpeg(EncodedJpeg {
                data,
                width,
                height,
                components,
            }))
        }
        Source::Raster(image) => {
            // An opaque raster is stored as JPEG even when it already fits the
            // budget: Flate over raw pixels is far larger for a scan, and a
            // re-encode at print resolution is indistinguishable on paper.
            // Real transparency still needs Flate + SMask, so it keeps that
            // path untouched.
            if let Some(budget) = budget {
                if alpha_is_opaque(&image) {
                    let jpeg = jpeg_from_dynamic(image, target, budget)?;
                    return Ok(ImageObject::Jpeg(jpeg));
                }
            }
            Ok(raster_object(image))
        }
    }
}

/// Read a file into the shape the rest of the pipeline expects.
///
/// JPEG keeps its bytes (and only its header is parsed, which is why a 12 MP
/// photo costs a few hundred microseconds here); anything else is decoded once
/// through `image`, without the extra per-pixel alpha split a generic loader
/// does.
fn load_source(path: &Path) -> Result<Source, ToolError> {
    let mut head = [0u8; 2];
    let mut file = File::open(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    let read = file.read(&mut head).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    drop(file);

    if read == 2 && head == [0xFF, 0xD8] {
        return match medpdf_image::load_image(path) {
            Ok(medpdf_image::ImageData::Jpeg {
                data,
                pixel_width,
                pixel_height,
                components,
            }) => Ok(Source::Jpeg {
                data,
                width: pixel_width,
                height: pixel_height,
                components,
            }),
            Ok(_) => Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "JPEG 扩展名与内容不一致".to_string(),
            }),
            Err(e) => Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: e.to_string(),
            }),
        };
    }

    let bytes = std::fs::read(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    image::load_from_memory(&bytes)
        .map(Source::Raster)
        .map_err(|e| ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: e.to_string(),
        })
}

/// Whether every pixel of `image` is (near) fully opaque.
///
/// Scanned pages are routinely exported with an alpha channel that is 255 in
/// every pixel; carrying it would double the file size for nothing.
fn alpha_is_opaque(image: &DynamicImage) -> bool {
    if !image.color().has_alpha() {
        return true;
    }
    match image {
        DynamicImage::ImageRgba8(buffer) => buffer.chunks_exact(4).all(|p| p[3] >= OPAQUE_ALPHA),
        DynamicImage::ImageLumaA8(buffer) => buffer.chunks_exact(2).all(|p| p[1] >= OPAQUE_ALPHA),
        // 16-bit variants are rare enough to take the converting path.
        other => other
            .to_rgba8()
            .chunks_exact(4)
            .all(|p| p[3] >= OPAQUE_ALPHA),
    }
}

/// Alpha at or above this counts as opaque (scan exports sometimes round 255
/// down by a bit).
const OPAQUE_ALPHA: u8 = 250;

/// Resize to `target` and encode as JPEG.
///
/// Catmull-Rom keeps scanned strokes crisp without the ringing Lanczos adds
/// around high-contrast text; grey sources stay grey, which is a third of the
/// bytes for the same page.
fn jpeg_from_dynamic(
    image: DynamicImage,
    target: (u32, u32),
    budget: ScanBudget,
) -> Result<EncodedJpeg, ToolError> {
    let gray = matches!(
        image.color(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::L16
            | image::ColorType::La16
    );
    let (tw, th) = target;
    let (raw, color) = if gray {
        let buffer = image::imageops::resize(&image.to_luma8(), tw, th, FilterType::CatmullRom);
        (buffer.into_raw(), ExtendedColorType::L8)
    } else {
        let buffer = image::imageops::resize(&image.to_rgb8(), tw, th, FilterType::CatmullRom);
        (buffer.into_raw(), ExtendedColorType::Rgb8)
    };
    encode_jpeg(raw, tw, th, color, budget.quality)
}

/// Flate/SMask payload for an image that must keep its transparency.
fn raster_object(image: DynamicImage) -> ImageObject {
    let width = image.width();
    let height = image.height();
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        let mut alpha = Vec::with_capacity((width * height) as usize);
        for pixel in rgba.pixels() {
            pixels.extend_from_slice(&pixel.0[..3]);
            alpha.push(pixel.0[3]);
        }
        ImageObject::Raster {
            pixels,
            alpha: Some(alpha),
            width,
            height,
            components: 3,
        }
    } else if matches!(
        image.color(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::L16
            | image::ColorType::La16
    ) {
        ImageObject::Raster {
            pixels: image.to_luma8().into_raw(),
            alpha: None,
            width,
            height,
            components: 1,
        }
    } else {
        ImageObject::Raster {
            pixels: image.to_rgb8().into_raw(),
            alpha: None,
            width,
            height,
            components: 3,
        }
    }
}

/// Pixel size to store an image at so it never exceeds `max_dpi` once drawn.
///
/// Never upscales: an image that is already within budget keeps its own
/// pixels, so the common case stays lossless.
fn target_pixels(px_w: u32, px_h: u32, drawn: (f32, f32), max_dpi: f32) -> (u32, u32) {
    let (draw_w, draw_h) = drawn;
    if px_w == 0 || px_h == 0 || draw_w <= 0.0 || draw_h <= 0.0 || max_dpi <= 0.0 {
        return (px_w, px_h);
    }
    // Both axes share one scale so the aspect ratio survives; the tighter of
    // the two effective DPIs decides.
    let effective = (px_w as f32 * 72.0 / draw_w).max(px_h as f32 * 72.0 / draw_h);
    if effective <= max_dpi {
        return (px_w, px_h);
    }
    let scale = max_dpi / effective;
    let width = ((px_w as f32 * scale).round() as u32).max(1);
    let height = ((px_h as f32 * scale).round() as u32).max(1);
    (width, height)
}

/// Encode raw samples at `quality`, reporting the payload's colour model.
fn encode_jpeg(
    raw: Vec<u8>,
    width: u32,
    height: u32,
    color: ExtendedColorType,
    quality: u8,
) -> Result<EncodedJpeg, ToolError> {
    let components = match color {
        ExtendedColorType::L8 => 1,
        _ => 3,
    };
    let mut data = Vec::new();
    JpegEncoder::new_with_quality(&mut data, quality.clamp(1, 100))
        .encode(&raw, width, height, color)
        .map_err(|e| ToolError::Internal {
            detail: format!("图片压缩失败：{e}"),
        })?;
    Ok(EncodedJpeg {
        data,
        width,
        height,
        components,
    })
}

/// Append one page holding `images` in draw order.
///
/// Each entry is `(resource name, prepared image)`; names must be unique per
/// page (`Im7`, `Im7-1`, …).
pub(crate) fn add_page(
    doc: &mut Document,
    pages_id: ObjectId,
    page: (f32, f32),
    images: Vec<(String, PreparedImage)>,
) -> Result<(), ToolError> {
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), page.0.into(), page.1.into()],
        "Resources" => dictionary! {},
    });
    attach_page(doc, pages_id, page_id)?;

    let mut draws: Vec<(String, [f32; 6])> = Vec::with_capacity(images.len());
    for (name, prepared) in images {
        let xobj_id = add_xobject(doc, prepared.object)?;
        register_xobject(doc, page_id, &name, xobj_id)?;
        draws.push((name, prepared.matrix));
    }
    write_content_stream(doc, page_id, &draws)
}

/// Write one image XObject.
fn add_xobject(doc: &mut Document, object: ImageObject) -> Result<ObjectId, ToolError> {
    match object {
        ImageObject::Jpeg(jpeg) => {
            let color_space = match jpeg.components {
                1 => "DeviceGray",
                4 => "DeviceCMYK",
                _ => "DeviceRGB",
            };
            let dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => jpeg.width as i64,
                "Height" => jpeg.height as i64,
                "ColorSpace" => color_space,
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            };
            let stream = lopdf::Stream::new(dict, jpeg.data).with_compression(false);
            Ok(doc.add_object(stream))
        }
        ImageObject::Raster {
            pixels,
            alpha,
            width,
            height,
            components,
        } => {
            let smask_id = match alpha {
                Some(alpha) => {
                    let mut smask = lopdf::Stream::new(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => width as i64,
                            "Height" => height as i64,
                            "ColorSpace" => "DeviceGray",
                            "BitsPerComponent" => 8,
                        },
                        alpha,
                    )
                    .with_compression(true);
                    // `save` never compresses on its own: without this the mask
                    // is written as raw bytes, one byte per pixel.
                    smask.compress().map_err(|e| ToolError::Internal {
                        detail: format!("透明度通道压缩失败：{e}"),
                    })?;
                    Some(doc.add_object(smask))
                }
                None => None,
            };

            let mut dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => width as i64,
                "Height" => height as i64,
                "ColorSpace" => if components == 1 { "DeviceGray" } else { "DeviceRGB" },
                "BitsPerComponent" => 8,
            };
            if let Some(smask_id) = smask_id {
                dict.set("SMask", Object::Reference(smask_id));
            }
            let mut stream = lopdf::Stream::new(dict, pixels).with_compression(true);
            // Same reason as the SMask: a raw 600 DPI page is ~100 MB, Flate
            // brings a scan back to a few megabytes.
            stream.compress().map_err(|e| ToolError::Internal {
                detail: format!("图片压缩失败：{e}"),
            })?;
            Ok(doc.add_object(stream))
        }
    }
}

/// Append a page to the page tree.
pub(crate) fn attach_page(
    doc: &mut Document,
    pages_id: ObjectId,
    page_id: ObjectId,
) -> Result<(), ToolError> {
    {
        let pages = doc
            .get_object_mut(pages_id)
            .and_then(|o| o.as_dict_mut())
            .map_err(|e| ToolError::Internal {
                detail: format!("页面树损坏：{e}"),
            })?;
        let kids = pages
            .get_mut(b"Kids")
            .and_then(|o| o.as_array_mut())
            .map_err(|e| ToolError::Internal {
                detail: format!("页面树 Kids 损坏：{e}"),
            })?;
        kids.push(Object::Reference(page_id));
        let count = kids.len() as i64;
        pages.set("Count", count);
    }
    doc.get_object_mut(page_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("页面对象损坏：{e}"),
        })?
        .set("Parent", Object::Reference(pages_id));
    Ok(())
}

/// Register an image XObject in a page's resources under `name`.
pub(crate) fn register_xobject(
    doc: &mut Document,
    page_id: ObjectId,
    name: &str,
    xobj_id: ObjectId,
) -> Result<(), ToolError> {
    medpdf::register_in_page_resources(doc, page_id, b"XObject", name.as_bytes(), xobj_id).map_err(
        |e| ToolError::Internal {
            detail: format!("注册图片资源失败：{e}"),
        },
    )
}

/// Write a page content stream drawing each `(name, matrix)` image in order.
pub(crate) fn write_content_stream(
    doc: &mut Document,
    page_id: ObjectId,
    images: &[(String, [f32; 6])],
) -> Result<(), ToolError> {
    let mut content = String::new();
    for (name, matrix) in images {
        content.push_str(&format!(
            "q\n{} {} {} {} {} {} cm\n/{name} Do\nQ\n",
            fmt_num(matrix[0]),
            fmt_num(matrix[1]),
            fmt_num(matrix[2]),
            fmt_num(matrix[3]),
            fmt_num(matrix[4]),
            fmt_num(matrix[5]),
        ));
    }
    let content_id = doc.add_object(lopdf::Stream::new(dictionary! {}, content.into_bytes()));
    doc.get_object_mut(page_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("写入页面内容失败：{e}"),
        })?
        .set("Contents", Object::Reference(content_id));
    Ok(())
}

/// Compose an extra 90° clockwise rotation onto an EXIF orientation code.
///
/// EXIF codes describe how the stored pixels must be transformed for display;
/// adding one more clockwise quarter-turn maps each code to the code that
/// produces the same final image, e.g. `1 -> 6` (rotate 90° CW).
pub(crate) fn rotate_cw_code(orientation: u8) -> u8 {
    match orientation {
        2 => 5,
        3 => 8,
        4 => 7,
        5 => 4,
        6 => 3,
        7 => 2,
        8 => 1,
        _ => 6,
    }
}

/// Map the unit square onto the page box, applying the EXIF orientation.
pub(crate) fn placement_matrix(orientation: u8, x0: f32, y0: f32, w: f32, h: f32) -> [f32; 6] {
    let x1 = x0 + w;
    let y1 = y0 + h;
    match orientation {
        2 => [-w, 0.0, 0.0, h, x1, y0],
        3 => [-w, 0.0, 0.0, -h, x1, y1],
        4 => [w, 0.0, 0.0, -h, x0, y1],
        5 => [0.0, h, w, 0.0, x0, y0],
        6 => [0.0, -h, w, 0.0, x0, y1],
        7 => [0.0, -h, -w, 0.0, x1, y1],
        8 => [0.0, h, -w, 0.0, x1, y0],
        _ => [w, 0.0, 0.0, h, x0, y0],
    }
}

/// Read the EXIF orientation tag, falling back to 1 (normal).
pub(crate) fn read_exif_orientation(path: &Path) -> u8 {
    let Ok(file) = File::open(path) else {
        return 1;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return 1;
    };
    match exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
        Some(field) => match field.value {
            exif::Value::Short(ref values) => {
                values.first().copied().unwrap_or(1).clamp(1, 8) as u8
            }
            _ => 1,
        },
        None => 1,
    }
}

/// Format a float compactly for a content stream.
pub(crate) fn fmt_num(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A4 portrait page box, the common case for a scanned form.
    const A4: (f32, f32) = (595.28, 841.89);

    #[test]
    fn target_pixels_drops_the_surplus_resolution() {
        // A 600 DPI A4 scan (4958 x 7017) only needs 2480 x 3508 at 300 DPI.
        let (w, h) = target_pixels(4958, 7017, A4, 300.0);
        assert!((w as i32 - 2480).abs() <= 1, "width was {w}");
        assert!((h as i32 - 3508).abs() <= 1, "height was {h}");
        // Aspect ratio survives the downscale.
        let before = 4958.0 / 7017.0;
        let after = w as f32 / h as f32;
        assert!((before - after).abs() < 0.002, "{before} vs {after}");
    }

    #[test]
    fn target_pixels_never_upscales() {
        // A 150 DPI scan is already below the 300 DPI budget: keep every pixel.
        assert_eq!(target_pixels(1240, 1754, A4, 300.0), (1240, 1754));
        // Degenerate geometry must not divide by zero or shrink to nothing.
        assert_eq!(target_pixels(800, 600, (0.0, 0.0), 300.0), (800, 600));
        assert_eq!(target_pixels(0, 0, A4, 300.0), (0, 0));
    }

    #[test]
    fn scan_budget_tightens_then_stops_at_the_floor() {
        let mut budget = ScanBudget::new(300.0, 85);
        let mut steps = 0;
        while let Some(next) = budget.tightened() {
            assert!(next.max_dpi < budget.max_dpi || next.quality < budget.quality);
            budget = next;
            steps += 1;
            assert!(steps < 32, "the ladder must terminate");
        }
        assert!((budget.max_dpi - MIN_DPI).abs() < 0.01, "{budget:?}");
        assert_eq!(budget.quality, MIN_QUALITY);
    }
}
