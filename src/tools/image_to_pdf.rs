//! Tool 1: merge images into a PDF.
//!
//! * JPEG inputs are embedded as `DCTDecode` **without re-encoding**; the
//!   original bytes are written straight into the image XObject.
//! * PNG inputs are decoded and re-compressed with Flate (documented trade-off).
//! * EXIF orientation is honoured by adjusting the placement matrix rather than
//!   by rotating pixels, which is what keeps the JPEG byte stream intact.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use medpdf_image::{ImageData, load_image};

use crate::engine::{pdf_io, validate};
use crate::model::{InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::{
    CancelFlag, Orientation, Progress, Tool, ToolOptions, ToolRequest, check_cancel,
};

/// A4 in PDF points (1 pt = 1/72 inch).
const A4_WIDTH: f32 = 595.28;
const A4_HEIGHT: f32 = 841.89;

/// Margin around each image, in points.
const PAGE_MARGIN: f32 = 0.0;

pub struct ImageToPdf;

impl Tool for ImageToPdf {
    fn id(&self) -> ToolId {
        ToolId::ImageToPdf
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        if req.inputs.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请先添加至少一张 JPEG 或 PNG 图片".to_string(),
            });
        }
        for input in &req.inputs {
            let ext = extension_of(&input.path)?;
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                return Err(ToolError::UnsupportedFormat {
                    path: Some(input.path.clone()),
                    detail: format!("仅支持 JPEG / PNG，收到 .{ext}"),
                });
            }
            if !input.path.is_file() {
                return Err(ToolError::ReadFailed {
                    path: input.path.clone(),
                    detail: "文件不存在".to_string(),
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
        let images = dedupe(req.inputs.clone());
        if images.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "没有可用的图片".to_string(),
            });
        }

        let force = match &req.options {
            ToolOptions::ImageToPdf { force_orientation } => *force_orientation,
            _ => None,
        };

        let mut doc = Document::with_version("1.7");
        let pages_id = build_document_skeleton(&mut doc);

        let total = images.len();
        let mut jpeg_count = 0usize;
        let mut skipped: Vec<(PathBuf, ToolError)> = Vec::new();

        for (index, input) in images.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("处理图片 {}/{}：{}", index + 1, total, input.display_name()),
            );

            match add_image_page(&mut doc, pages_id, &input.path, force, index) {
                Ok(is_jpeg) => {
                    if is_jpeg {
                        jpeg_count += 1;
                    }
                }
                Err(err) => {
                    // One bad image must not abort the batch; record and continue.
                    skipped.push((input.path.clone(), err));
                }
            }
        }

        if page_count_in(&doc) == 0 {
            let detail = skipped
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有任何图片可用（{detail}）"),
            });
        }

        progress.report(0.95, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &req.output)?;

        let expected_pages = page_count_in(&doc);
        validate::assert_page_count(&req.output, expected_pages, None)?;
        if jpeg_count > 0 {
            validate::assert_dctdecode_count(&req.output, jpeg_count)?;
            // Raw SOI markers must still be present, proving no re-encode happened.
            validate::assert_jpeg_soi_bytes(&req.output, jpeg_count)?;
        }

        progress.report(1.0, "完成".to_string());

        let mut summary = format!("已生成 {expected_pages} 页 PDF（JPEG 直通 {jpeg_count} 张）");
        if !skipped.is_empty() {
            summary.push_str(&format!("，跳过 {} 张", skipped.len()));
            return Err(ToolError::PartialSuccess {
                succeeded: expected_pages as usize,
                failed: skipped.len(),
                detail: format!("{summary}；{}", describe_skips(&skipped)),
            });
        }

        Ok(ToolOutcome::success(summary, vec![req.output.clone()]))
    }
}

/// Remove duplicate inputs while preserving order (canonicalised where possible).
fn dedupe(inputs: Vec<InputFile>) -> Vec<InputFile> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::with_capacity(inputs.len());
    for input in inputs {
        let key = input
            .path
            .canonicalize()
            .unwrap_or_else(|_| input.path.clone());
        if seen.insert(key) {
            out.push(input);
        }
    }
    out
}

fn describe_skips(skipped: &[(PathBuf, ToolError)]) -> String {
    skipped
        .iter()
        .map(|(path, err)| format!("{}: {err}", path.display()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn extension_of(path: &Path) -> Result<String, ToolError> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .ok_or_else(|| ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "文件没有扩展名".to_string(),
        })
}

/// Minimal but valid single-page-tree document.
fn build_document_skeleton(doc: &mut Document) -> ObjectId {
    let pages_id = doc.new_object_id();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => Vec::<Object>::new(),
            "Count" => 0i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    pages_id
}

fn page_count_in(doc: &Document) -> u32 {
    doc.get_pages().len() as u32
}

/// Add one page holding `path`, returning whether the source was a JPEG.
fn add_image_page(
    doc: &mut Document,
    pages_id: ObjectId,
    path: &Path,
    force: Option<Orientation>,
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

    let landscape = match force {
        Some(Orientation::Landscape) => true,
        Some(Orientation::Portrait) => false,
        None => eff_w > eff_h,
    };
    let (page_w, page_h) = if landscape {
        (A4_HEIGHT, A4_WIDTH)
    } else {
        (A4_WIDTH, A4_HEIGHT)
    };

    // Fit the image into the page box, preserving aspect ratio.
    let box_w = page_w - PAGE_MARGIN * 2.0;
    let box_h = page_h - PAGE_MARGIN * 2.0;
    let scale = (box_w / eff_w).min(box_h / eff_h);
    let draw_w = eff_w * scale;
    let draw_h = eff_h * scale;
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
    medpdf::register_in_page_resources(
        doc,
        page_id,
        b"XObject",
        name.as_bytes(),
        xobj_id,
    )
    .map_err(|e| ToolError::Internal {
        detail: format!("注册图片资源失败：{e}"),
    })?;

    let matrix = placement_matrix(orientation, x0, y0, draw_w, draw_h);
    let content = format!(
        "q\n{} {} {} {} {} {} cm\n/{} Do\nQ\n",
        f(matrix[0]),
        f(matrix[1]),
        f(matrix[2]),
        f(matrix[3]),
        f(matrix[4]),
        f(matrix[5]),
        name
    );
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    doc.get_object_mut(page_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("写入页面内容失败：{e}"),
        })?
        .set("Contents", Object::Reference(content_id));

    Ok(is_jpeg)
}

fn attach_page(doc: &mut Document, pages_id: ObjectId, page_id: ObjectId) -> Result<(), ToolError> {
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
fn add_image_xobject(doc: &mut Document, image: ImageData) -> Result<ObjectId, ToolError> {
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
            let stream = Stream::new(dict, data).with_compression(false);
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
                    let smask = Stream::new(
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
            let stream = Stream::new(dict, pixels).with_compression(true);
            Ok(doc.add_object(stream))
        }
    }
}

/// Map the unit square onto the page box, applying the EXIF orientation.
fn placement_matrix(orientation: u8, x0: f32, y0: f32, w: f32, h: f32) -> [f32; 6] {
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
fn read_exif_orientation(path: &Path) -> u8 {
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
fn f(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}
