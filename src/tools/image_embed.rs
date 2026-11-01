//! Shared image-embedding plumbing for the two image→PDF tools.
//!
//! * JPEG inputs are embedded as `DCTDecode` **without re-encoding**; the
//!   original bytes are written straight into the image XObject.
//! * PNG inputs are decoded and re-compressed with Flate (documented trade-off).
//! * EXIF orientation is honoured by adjusting the placement matrix rather than
//!   by rotating pixels, which is what keeps the JPEG byte stream intact.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId};
use medpdf_image::{load_image, ImageData};

use crate::model::ToolError;
use crate::tools::Orientation;

/// A4 in PDF points (1 pt = 1/72 inch).
pub(crate) const A4_WIDTH: f32 = 595.28;
pub(crate) const A4_HEIGHT: f32 = 841.89;

/// Margin around each image, in points.
pub(crate) const PAGE_MARGIN: f32 = 0.0;

/// Add one A4 page holding `path`, returning whether the source was a JPEG.
///
/// `force` fixes the page orientation; with `auto_rotate`, an image whose own
/// orientation conflicts with the forced page orientation is turned 90° so it
/// fills the page instead of appearing as a small centred strip.
pub(crate) fn add_image_page(
    doc: &mut Document,
    pages_id: ObjectId,
    path: &Path,
    force: Option<Orientation>,
    auto_rotate: bool,
    index: usize,
) -> Result<bool, ToolError> {
    let image = load_image(path).map_err(|e| ToolError::UnsupportedFormat {
        path: Some(path.to_path_buf()),
        detail: e.to_string(),
    })?;

    let is_jpeg = matches!(image, ImageData::Jpeg { .. });
    let orientation = if is_jpeg { read_exif_orientation(path) } else { 1 };

    let (px_w, px_h) = (image.pixel_width() as f32, image.pixel_height() as f32);
    if px_w <= 0.0 || px_h <= 0.0 {
        return Err(ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "图片尺寸为 0".to_string(),
        });
    }

    // Orientations 5..=8 display the image rotated by 90 degrees, so the
    // effective width/height are swapped.
    let (eff_w, eff_h) = if matches!(orientation, 5 | 6 | 7 | 8) {
        (px_h, px_w)
    } else {
        (px_w, px_h)
    };

    let image_is_landscape = eff_w > eff_h;
    let landscape_page = match force {
        Some(Orientation::Landscape) => true,
        Some(Orientation::Portrait) => false,
        None => image_is_landscape,
    };

    // When the forced page orientation conflicts with the image's own
    // orientation, optionally rotate the image 90° so it fills the page
    // instead of appearing as a small centred strip.
    let rotate = auto_rotate && force.is_some() && image_is_landscape != landscape_page;
    let (fit_w, fit_h) = if rotate {
        (eff_h, eff_w)
    } else {
        (eff_w, eff_h)
    };

    let (page_w, page_h) = if landscape_page {
        (A4_HEIGHT, A4_WIDTH)
    } else {
        (A4_WIDTH, A4_HEIGHT)
    };

    // Fit the image into the page box, preserving aspect ratio.
    let box_w = page_w - PAGE_MARGIN * 2.0;
    let box_h = page_h - PAGE_MARGIN * 2.0;
    let scale = (box_w / fit_w).min(box_h / fit_h);
    let draw_w = fit_w * scale;
    let draw_h = fit_h * scale;
    let x0 = (page_w - draw_w) / 2.0;
    let y0 = (page_h - draw_h) / 2.0;

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), page_w.into(), page_h.into()],
        "Resources" => dictionary! {},
    });
    attach_page(doc, pages_id, page_id)?;

    let name = format!("Im{}", index + 1);
    let xobj_id = add_image_xobject(doc, image)?;
    register_xobject(doc, page_id, &name, xobj_id)?;

    // The final placement code composes the EXIF orientation with the extra
    // 90° rotation, so one matrix lookup covers both.
    let code = if rotate {
        rotate_cw_code(orientation)
    } else {
        orientation
    };
    let matrix = placement_matrix(code, x0, y0, draw_w, draw_h);
    write_content_stream(
        doc,
        page_id,
        &[(name, matrix)],
    )?;

    Ok(is_jpeg)
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

/// Build the image XObject, keeping JPEG bytes verbatim.
pub(crate) fn add_image_xobject(
    doc: &mut Document,
    image: ImageData,
) -> Result<ObjectId, ToolError> {
    match image {
        ImageData::Jpeg {
            data,
            pixel_width,
            pixel_height,
            components,
        } => {
            let color_space = match components {
                1 => "DeviceGray",
                4 => "DeviceCMYK",
                _ => "DeviceRGB",
            };
            let dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => pixel_width as i64,
                "Height" => pixel_height as i64,
                "ColorSpace" => color_space,
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            };
            let stream = lopdf::Stream::new(dict, data).with_compression(false);
            Ok(doc.add_object(stream))
        }
        ImageData::Decoded {
            pixels,
            alpha_channel,
            pixel_width,
            pixel_height,
            components,
        } => {
            let smask_id = match alpha_channel {
                Some(alpha) => {
                    let smask = lopdf::Stream::new(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => pixel_width as i64,
                            "Height" => pixel_height as i64,
                            "ColorSpace" => "DeviceGray",
                            "BitsPerComponent" => 8,
                        },
                        alpha,
                    )
                    .with_compression(true);
                    Some(doc.add_object(smask))
                }
                None => None,
            };

            let color_space = if components == 1 { "DeviceGray" } else { "DeviceRGB" };
            let mut dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => pixel_width as i64,
                "Height" => pixel_height as i64,
                "ColorSpace" => color_space,
                "BitsPerComponent" => 8,
            };
            if let Some(smask_id) = smask_id {
                dict.set("SMask", Object::Reference(smask_id));
            }
            let stream = lopdf::Stream::new(dict, pixels).with_compression(true);
            Ok(doc.add_object(stream))
        }
    }
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
            exif::Value::Short(ref values) => values.first().copied().unwrap_or(1).clamp(1, 8) as u8,
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
