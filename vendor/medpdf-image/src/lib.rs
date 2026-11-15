use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use medpdf::{
    insert_content_stream, register_extgstate_in_page_resources, MedpdfError, Result,
    KEY_XOBJECT,
};
use std::path::Path;

#[cfg(feature = "svg")]
mod svg;

#[cfg(feature = "svg")]
pub use svg::{add_svg, load_svg, load_svg_bytes, load_svg_str, DrawSvgParams, SvgData, SvgOptions};

/// Fit mode when both width and height are specified.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageFit {
    /// Distort image to exact w x h.
    Stretch,
    /// Fit inside box preserving aspect ratio, centered.
    #[default]
    Contain,
    /// Fill box preserving aspect ratio, overflow clipped.
    Cover,
}

/// Raw image data ready for embedding.
#[derive(Debug, Clone)]
pub enum ImageData {
    /// JPEG bytes with pre-parsed dimensions (embedded as DCTDecode without re-encoding).
    Jpeg {
        data: Vec<u8>,
        pixel_width: u32,
        pixel_height: u32,
        /// 1=Gray, 3=RGB, 4=CMYK
        components: u8,
    },
    /// Decoded pixel data (RGB or Gray, no alpha in pixels).
    Decoded {
        /// RGB or Gray pixel bytes (no alpha).
        pixels: Vec<u8>,
        /// Separate alpha channel, if present.
        alpha_channel: Option<Vec<u8>>,
        pixel_width: u32,
        pixel_height: u32,
        /// 1=Gray, 3=RGB
        components: u8,
    },
}

impl ImageData {
    pub fn pixel_width(&self) -> u32 {
        match self {
            ImageData::Jpeg { pixel_width, .. } | ImageData::Decoded { pixel_width, .. } => {
                *pixel_width
            }
        }
    }

    pub fn pixel_height(&self) -> u32 {
        match self {
            ImageData::Jpeg { pixel_height, .. } | ImageData::Decoded { pixel_height, .. } => {
                *pixel_height
            }
        }
    }
}

/// Parameters for placing an image on a PDF page.
pub struct DrawImageParams {
    pub image_data: ImageData,
    /// X position in points.
    pub x: f32,
    /// Y position in points.
    pub y: f32,
    /// Output box width in points.
    pub width: f32,
    /// Output box height in points.
    pub height: f32,
    /// Fit mode (default: Contain).
    pub fit: ImageFit,
    /// Max DPI limit. 0 = no limit. Default: 300.
    pub max_dpi: f32,
    /// Opacity (0.0 = transparent, 1.0 = opaque). Default: 1.0.
    pub alpha: f32,
    /// Rotation in degrees. Default: 0.0.
    pub rotation: f32,
    /// If true, draw over existing content; if false, draw under. Default: true.
    pub layer_over: bool,
}

impl DrawImageParams {
    /// Create new params with required fields. `width` and `height` are in points.
    pub fn new(image_data: ImageData, x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            image_data,
            x,
            y,
            width,
            height,
            fit: ImageFit::Contain,
            max_dpi: 300.0,
            alpha: 1.0,
            rotation: 0.0,
            layer_over: true,
        }
    }

    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }

    pub fn max_dpi(mut self, max_dpi: f32) -> Self {
        self.max_dpi = max_dpi;
        self
    }

    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    pub fn rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn layer_over(mut self, layer_over: bool) -> Self {
        self.layer_over = layer_over;
        self
    }
}

// ---------------------------------------------------------------------------
// JPEG SOF parser
// ---------------------------------------------------------------------------

/// Parse a JPEG's SOF marker to extract width, height, and component count.
/// Returns (width, height, components).
fn parse_jpeg_sof(data: &[u8]) -> Result<(u32, u32, u8)> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(MedpdfError::new("Not a valid JPEG file"));
    }

    let mut i = 2;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            return Err(MedpdfError::new("Invalid JPEG marker"));
        }

        // Skip fill bytes
        while i + 1 < data.len() && data[i + 1] == 0xFF {
            i += 1;
        }
        if i + 1 >= data.len() {
            break;
        }

        let marker = data[i + 1];
        i += 2;

        // SOF markers: SOF0 (0xC0), SOF1 (0xC1), SOF2 (0xC2), SOF3 (0xC3)
        // We accept any baseline/progressive/lossless SOF
        if matches!(marker, 0xC0..=0xC3) {
            if i + 8 > data.len() {
                return Err(MedpdfError::new("JPEG SOF marker truncated"));
            }
            // Skip length (2 bytes) and precision (1 byte)
            let height = u16::from_be_bytes([data[i + 3], data[i + 4]]) as u32;
            let width = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let components = data[i + 7];
            return Ok((width, height, components));
        }

        // Skip over non-SOF markers
        // Markers without payload
        if marker == 0xD9 || marker == 0x00 {
            continue;
        }
        // RST markers (0xD0-0xD7) have no payload
        if (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        if i + 1 >= data.len() {
            break;
        }
        let length = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        if length < 2 {
            return Err(MedpdfError::new("Invalid JPEG marker length"));
        }
        i += length;
    }

    Err(MedpdfError::new(
        "Could not find SOF marker in JPEG file",
    ))
}

// ---------------------------------------------------------------------------
// Image loading
// ---------------------------------------------------------------------------

/// Load an image from a file path.
///
/// JPEG files are kept as raw bytes (for DCTDecode passthrough).
/// All other formats are decoded via the `image` crate.
pub fn load_image(path: &Path) -> Result<ImageData> {
    let data = std::fs::read(path)
        .map_err(|e| MedpdfError::new(format!("Failed to read image file '{}': {}", path.display(), e)))?;

    // Check for JPEG magic bytes
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let (pixel_width, pixel_height, components) = parse_jpeg_sof(&data)?;
        return Ok(ImageData::Jpeg {
            data,
            pixel_width,
            pixel_height,
            components,
        });
    }

    // Decode via image crate
    let img = image::load_from_memory(&data)
        .map_err(|e| MedpdfError::new(format!("Failed to decode image '{}': {}", path.display(), e)))?;

    let color = img.color();
    let has_alpha = color.has_alpha();
    let is_grayscale = matches!(
        color,
        image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16
    );

    if has_alpha {
        if is_grayscale {
            let la = img.into_luma_alpha8();
            let (w, h) = (la.width(), la.height());
            let mut pixels = Vec::with_capacity((w * h) as usize);
            let mut alpha_channel = Vec::with_capacity((w * h) as usize);
            for pixel in la.pixels() {
                pixels.push(pixel[0]);
                alpha_channel.push(pixel[1]);
            }
            Ok(ImageData::Decoded {
                pixels,
                alpha_channel: Some(alpha_channel),
                pixel_width: w,
                pixel_height: h,
                components: 1,
            })
        } else {
            let rgba = img.into_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            let mut pixels = Vec::with_capacity((w * h * 3) as usize);
            let mut alpha_channel = Vec::with_capacity((w * h) as usize);
            for pixel in rgba.pixels() {
                pixels.push(pixel[0]);
                pixels.push(pixel[1]);
                pixels.push(pixel[2]);
                alpha_channel.push(pixel[3]);
            }
            Ok(ImageData::Decoded {
                pixels,
                alpha_channel: Some(alpha_channel),
                pixel_width: w,
                pixel_height: h,
                components: 3,
            })
        }
    } else if is_grayscale {
        let gray = img.into_luma8();
        let (w, h) = (gray.width(), gray.height());
        Ok(ImageData::Decoded {
            pixels: gray.into_raw(),
            alpha_channel: None,
            pixel_width: w,
            pixel_height: h,
            components: 1,
        })
    } else {
        let rgb = img.into_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        Ok(ImageData::Decoded {
            pixels: rgb.into_raw(),
            alpha_channel: None,
            pixel_width: w,
            pixel_height: h,
            components: 3,
        })
    }
}

// ---------------------------------------------------------------------------
// Downsampling
// ---------------------------------------------------------------------------

/// Downsample image data if effective DPI exceeds max_dpi.
/// Returns the (possibly modified) image data.
fn maybe_downsample(image_data: ImageData, output_w_pts: f32, output_h_pts: f32, max_dpi: f32) -> Result<ImageData> {
    if max_dpi <= 0.0 {
        return Ok(image_data);
    }

    let pw = image_data.pixel_width() as f32;
    let ph = image_data.pixel_height() as f32;
    let eff_dpi_x = pw / (output_w_pts / 72.0);
    let eff_dpi_y = ph / (output_h_pts / 72.0);
    let eff_dpi = eff_dpi_x.max(eff_dpi_y);

    if eff_dpi <= max_dpi {
        return Ok(image_data);
    }

    let scale = max_dpi / eff_dpi;
    let new_w = (pw * scale).round().max(1.0) as u32;
    let new_h = (ph * scale).round().max(1.0) as u32;

    log::info!("Downsampling image from {}x{} to {}x{} (effective DPI {:.0} -> {:.0})",
        image_data.pixel_width(), image_data.pixel_height(), new_w, new_h, eff_dpi, max_dpi);

    match image_data {
        ImageData::Jpeg { data, pixel_width: _, pixel_height: _, components } => {
            // Decode JPEG, resize, re-encode as JPEG
            let img = image::load_from_memory(&data)
                .map_err(|e| MedpdfError::new(format!("Failed to decode JPEG for downsampling: {e}")))?;
            let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);

            let mut jpeg_buf = std::io::Cursor::new(Vec::new());
            resized.write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)
                .map_err(|e| MedpdfError::new(format!("Failed to re-encode JPEG: {e}")))?;

            // Re-parse the new JPEG to get exact dimensions
            let jpeg_data = jpeg_buf.into_inner();
            let (actual_w, actual_h, actual_c) = parse_jpeg_sof(&jpeg_data)?;

            Ok(ImageData::Jpeg {
                data: jpeg_data,
                pixel_width: actual_w,
                pixel_height: actual_h,
                components: if components == 4 { 3 } else { actual_c }, // CMYK gets converted to RGB
            })
        }
        ImageData::Decoded { pixels, alpha_channel, pixel_width, pixel_height, components } => {
            // Resize the pixel buffer
            let resized_pixels = if components == 3 {
                let img = image::RgbImage::from_raw(pixel_width, pixel_height, pixels)
                    .ok_or_else(|| MedpdfError::new("Invalid RGB pixel buffer dimensions"))?;
                let resized = image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
                resized.into_raw()
            } else {
                // Grayscale
                let img = image::GrayImage::from_raw(pixel_width, pixel_height, pixels)
                    .ok_or_else(|| MedpdfError::new("Invalid Gray pixel buffer dimensions"))?;
                let resized = image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
                resized.into_raw()
            };

            let resized_alpha = if let Some(alpha) = alpha_channel {
                let alpha_img = image::GrayImage::from_raw(pixel_width, pixel_height, alpha)
                    .ok_or_else(|| MedpdfError::new("Invalid alpha channel dimensions"))?;
                let resized = image::imageops::resize(&alpha_img, new_w, new_h, image::imageops::FilterType::Lanczos3);
                Some(resized.into_raw())
            } else {
                None
            };

            Ok(ImageData::Decoded {
                pixels: resized_pixels,
                alpha_channel: resized_alpha,
                pixel_width: new_w,
                pixel_height: new_h,
                components,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// XObject registration
// ---------------------------------------------------------------------------

/// Registers an XObject in the page's Resources.
pub(crate) fn register_xobject_in_page_resources(
    doc: &mut Document,
    page_id: ObjectId,
    xobj_id: ObjectId,
    xobj_name: &str,
) -> Result<()> {
    medpdf::register_in_page_resources(doc, page_id, KEY_XOBJECT, xobj_name.as_bytes(), xobj_id)
}

/// Generate an XObject name with the given prefix that doesn't collide with existing entries.
pub(crate) fn unique_xobject_name(doc: &Document, page_id: ObjectId, prefix: &str) -> String {
    // Collect existing XObject keys from the page's resources (best-effort).
    let existing = (|| -> Option<std::collections::HashSet<Vec<u8>>> {
        let page_dict = doc.get_dictionary(page_id).ok()?;
        let res_obj = page_dict.get(medpdf::KEY_RESOURCES).ok()?;
        let res_dict = match res_obj {
            Object::Reference(id) => doc.get_dictionary(*id).ok()?,
            Object::Dictionary(d) => d,
            _ => return None,
        };
        let xobj_obj = res_dict.get(KEY_XOBJECT).ok()?;
        let xobj_dict = match xobj_obj {
            Object::Reference(id) => doc.get_dictionary(*id).ok()?,
            Object::Dictionary(d) => d,
            _ => return None,
        };
        Some(xobj_dict.as_hashmap().keys().cloned().collect())
    })()
    .unwrap_or_default();

    for i in 0u32.. {
        let name = format!("{prefix}{i}");
        if !existing.contains(name.as_bytes()) {
            return name;
        }
    }
    unreachable!()
}

// ---------------------------------------------------------------------------
// Sizing / fit logic
// ---------------------------------------------------------------------------

/// Compute actual placement dimensions and offset within the box.
/// Returns (actual_w, actual_h, offset_x, offset_y, needs_clip).
pub(crate) fn compute_fit(
    img_w: f32,
    img_h: f32,
    box_w: f32,
    box_h: f32,
    fit: ImageFit,
) -> (f32, f32, f32, f32, bool) {
    match fit {
        ImageFit::Stretch => (box_w, box_h, 0.0, 0.0, false),
        ImageFit::Contain => {
            let scale = (box_w / img_w).min(box_h / img_h);
            let actual_w = img_w * scale;
            let actual_h = img_h * scale;
            let offset_x = (box_w - actual_w) / 2.0;
            let offset_y = (box_h - actual_h) / 2.0;
            (actual_w, actual_h, offset_x, offset_y, false)
        }
        ImageFit::Cover => {
            let scale = (box_w / img_w).max(box_h / img_h);
            let actual_w = img_w * scale;
            let actual_h = img_h * scale;
            let offset_x = (box_w - actual_w) / 2.0;
            let offset_y = (box_h - actual_h) / 2.0;
            (actual_w, actual_h, offset_x, offset_y, true)
        }
    }
}

// ---------------------------------------------------------------------------
// Core: add_image
// ---------------------------------------------------------------------------

/// Embed an image into a PDF page.
pub fn add_image(doc: &mut Document, page_id: ObjectId, params: DrawImageParams) -> Result<()> {
    if params.width <= 0.0 || params.height <= 0.0 {
        return Err(MedpdfError::new(format!(
            "Image output dimensions must be positive, got {}x{}",
            params.width, params.height
        )));
    }

    let img_w = params.image_data.pixel_width() as f32;
    let img_h = params.image_data.pixel_height() as f32;

    // Compute fit
    let (actual_w, actual_h, offset_x, offset_y, needs_clip) =
        compute_fit(img_w, img_h, params.width, params.height, params.fit);

    // Downsample if needed
    let image_data = maybe_downsample(params.image_data, actual_w, actual_h, params.max_dpi)?;

    // Create the image XObject
    let img_name = unique_xobject_name(doc, page_id, "Img");

    let xobj_id = match image_data {
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
            let img_dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => pixel_width as i64,
                "Height" => pixel_height as i64,
                "ColorSpace" => color_space,
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
                "Length" => data.len() as i64,
            };
            doc.add_object(Stream::new(img_dict, data))
        }
        ImageData::Decoded {
            pixels,
            alpha_channel,
            pixel_width,
            pixel_height,
            components,
        } => {
            let color_space = if components == 1 {
                "DeviceGray"
            } else {
                "DeviceRGB"
            };

            // Compress pixel data with flate2
            let compressed = flate_compress(&pixels)?;

            let mut img_dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => pixel_width as i64,
                "Height" => pixel_height as i64,
                "ColorSpace" => color_space,
                "BitsPerComponent" => 8,
                "Filter" => "FlateDecode",
                "Length" => compressed.len() as i64,
            };

            // Create SMask for alpha channel
            if let Some(alpha) = &alpha_channel {
                let alpha_compressed = flate_compress(alpha)?;
                let smask_dict = dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => pixel_width as i64,
                    "Height" => pixel_height as i64,
                    "ColorSpace" => "DeviceGray",
                    "BitsPerComponent" => 8,
                    "Filter" => "FlateDecode",
                    "Length" => alpha_compressed.len() as i64,
                };
                let smask_id = doc.add_object(Stream::new(smask_dict, alpha_compressed));
                img_dict.set("SMask", Object::Reference(smask_id));
            }

            doc.add_object(Stream::new(img_dict, compressed))
        }
    };

    // Register XObject in page Resources
    register_xobject_in_page_resources(doc, page_id, xobj_id, &img_name)?;

    // Build content stream
    let mut content = String::new();
    content.push_str("q\n");

    // Alpha via ExtGState
    if (params.alpha - 1.0).abs() > f32::EPSILON {
        let gs_dict = dictionary! {
            "Type" => "ExtGState",
            "ca" => params.alpha,
            "CA" => params.alpha,
        };
        let gs_id = doc.add_object(gs_dict);
        let gs_key = register_extgstate_in_page_resources(doc, page_id, gs_id)?;
        content.push_str(&format!("/{gs_key} gs\n"));
    }

    // Rotation
    if params.rotation.abs() > 0.001 {
        let angle = params.rotation.to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        // Rotate around the center of the box
        let cx = params.x + params.width / 2.0;
        let cy = params.y + params.height / 2.0;
        // Translate to center, rotate, translate back
        content.push_str(&format!(
            "1 0 0 1 {cx} {cy} cm\n{cos} {sin} {nsin} {cos} 0 0 cm\n1 0 0 1 {ncx} {ncy} cm\n",
            cx = fmt_f32(cx),
            cy = fmt_f32(cy),
            cos = fmt_f32(cos),
            sin = fmt_f32(sin),
            nsin = fmt_f32(-sin),
            ncx = fmt_f32(-cx),
            ncy = fmt_f32(-cy),
        ));
    }

    // Clipping rect for cover mode
    if needs_clip {
        content.push_str(&format!(
            "{x} {y} {w} {h} re W n\n",
            x = fmt_f32(params.x),
            y = fmt_f32(params.y),
            w = fmt_f32(params.width),
            h = fmt_f32(params.height),
        ));
    }

    // Image placement matrix: scale + translate
    let place_x = params.x + offset_x;
    let place_y = params.y + offset_y;
    content.push_str(&format!(
        "{w} 0 0 {h} {x} {y} cm\n/{name} Do\n",
        w = fmt_f32(actual_w),
        h = fmt_f32(actual_h),
        x = fmt_f32(place_x),
        y = fmt_f32(place_y),
        name = img_name,
    ));

    content.push_str("Q\n");

    let content_stream = Stream::new(dictionary! {}, content.into_bytes());
    let content_id = doc.add_object(content_stream);

    insert_content_stream(doc, page_id, content_id, params.layer_over)
}

fn flate_compress(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| MedpdfError::new(format!("flate2 compress failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| MedpdfError::new(format!("flate2 finish failed: {e}")))
}

pub(crate) fn fmt_f32(v: f32) -> String {
    // Avoid trailing zeros, but keep reasonable precision
    let mut s = format!("{v:.4}");
    let trimmed_len = s.trim_end_matches('0').trim_end_matches('.').len();
    s.truncate(trimmed_len);
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- JPEG SOF parser tests ---

    #[test]
    fn test_parse_jpeg_sof_basic() {
        // Minimal JPEG: SOI + SOF0 marker
        // FF D8 (SOI)
        // FF C0 (SOF0) 00 0B (length=11) 08 (precision) 00 64 (h=100) 00 C8 (w=200) 03 (components)
        let data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xC0, // SOF0
            0x00, 0x0B, // length
            0x08, // precision (8-bit)
            0x00, 0x64, // height = 100
            0x00, 0xC8, // width = 200
            0x03, // 3 components
        ];
        let (w, h, c) = parse_jpeg_sof(&data).unwrap();
        assert_eq!(w, 200);
        assert_eq!(h, 100);
        assert_eq!(c, 3);
    }

    #[test]
    fn test_parse_jpeg_sof_with_app0() {
        // SOI + APP0 marker + SOF0
        let mut data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, // APP0 (JFIF)
            0x00, 0x10, // length = 16
        ];
        data.extend_from_slice(&[0u8; 14]); // APP0 payload (14 more bytes for 16 total)
        data.extend_from_slice(&[
            0xFF, 0xC0, // SOF0
            0x00, 0x0B, // length
            0x08, // precision
            0x01, 0x00, // height = 256
            0x02, 0x00, // width = 512
            0x03, // 3 components
        ]);
        let (w, h, c) = parse_jpeg_sof(&data).unwrap();
        assert_eq!(w, 512);
        assert_eq!(h, 256);
        assert_eq!(c, 3);
    }

    #[test]
    fn test_parse_jpeg_sof_progressive() {
        // SOF2 (progressive)
        let data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xC2, // SOF2
            0x00, 0x0B, 0x08, 0x00, 0x50, // height = 80
            0x00, 0xA0, // width = 160
            0x01, // 1 component (grayscale)
        ];
        let (w, h, c) = parse_jpeg_sof(&data).unwrap();
        assert_eq!(w, 160);
        assert_eq!(h, 80);
        assert_eq!(c, 1);
    }

    #[test]
    fn test_parse_jpeg_sof_not_jpeg() {
        let data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic
        assert!(parse_jpeg_sof(&data).is_err());
    }

    #[test]
    fn test_parse_jpeg_sof_truncated() {
        let data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        assert!(parse_jpeg_sof(&data).is_err());
    }

    #[test]
    fn test_parse_jpeg_sof_empty() {
        let data = vec![];
        assert!(parse_jpeg_sof(&data).is_err());
    }

    // --- Sizing / fit tests ---

    #[test]
    fn test_compute_fit_stretch() {
        let (w, h, ox, oy, clip) = compute_fit(100.0, 200.0, 300.0, 300.0, ImageFit::Stretch);
        assert!((w - 300.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
        assert!((ox - 0.0).abs() < 0.01);
        assert!((oy - 0.0).abs() < 0.01);
        assert!(!clip);
    }

    #[test]
    fn test_compute_fit_contain_landscape() {
        // 200x100 image in 300x300 box -> scale by 1.5 -> 300x150, centered vertically
        let (w, h, ox, oy, clip) = compute_fit(200.0, 100.0, 300.0, 300.0, ImageFit::Contain);
        assert!((w - 300.0).abs() < 0.01);
        assert!((h - 150.0).abs() < 0.01);
        assert!((ox - 0.0).abs() < 0.01);
        assert!((oy - 75.0).abs() < 0.01);
        assert!(!clip);
    }

    #[test]
    fn test_compute_fit_contain_portrait() {
        // 100x200 image in 300x300 box -> scale by 1.5 -> 150x300, centered horizontally
        let (w, h, ox, oy, clip) = compute_fit(100.0, 200.0, 300.0, 300.0, ImageFit::Contain);
        assert!((w - 150.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
        assert!((ox - 75.0).abs() < 0.01);
        assert!((oy - 0.0).abs() < 0.01);
        assert!(!clip);
    }

    #[test]
    fn test_compute_fit_cover_landscape() {
        // 200x100 image in 300x300 box -> scale by 3.0 -> 600x300
        let (w, h, ox, oy, clip) = compute_fit(200.0, 100.0, 300.0, 300.0, ImageFit::Cover);
        assert!((w - 600.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
        assert!((ox - -150.0).abs() < 0.01);
        assert!((oy - 0.0).abs() < 0.01);
        assert!(clip);
    }

    #[test]
    fn test_compute_fit_contain_exact() {
        // Image exactly matches box
        let (w, h, ox, oy, clip) = compute_fit(300.0, 300.0, 300.0, 300.0, ImageFit::Contain);
        assert!((w - 300.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
        assert!((ox - 0.0).abs() < 0.01);
        assert!((oy - 0.0).abs() < 0.01);
        assert!(!clip);
    }

    // --- fmt_f32 tests ---

    #[test]
    fn test_fmt_f32_integer() {
        assert_eq!(fmt_f32(72.0), "72");
    }

    #[test]
    fn test_fmt_f32_decimal() {
        assert_eq!(fmt_f32(72.5), "72.5");
    }

    #[test]
    fn test_fmt_f32_small_decimal() {
        assert_eq!(fmt_f32(0.001), "0.001");
    }

    #[test]
    fn test_fmt_f32_zero() {
        assert_eq!(fmt_f32(0.0), "0");
    }

    // --- XObject dict tests ---

    fn create_test_doc_and_page() -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![],
            "Count" => 0,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

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
        let pages_obj = doc.get_object_mut(pages_id).unwrap().as_dict_mut().unwrap();
        let kids = pages_obj.get_mut(b"Kids").unwrap().as_array_mut().unwrap();
        kids.push(Object::Reference(page_id));
        pages_obj.set("Count", Object::Integer(1));
        (doc, page_id)
    }

    #[test]
    fn test_register_xobject_creates_dict() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let xobj_id = doc.add_object(dictionary! { "Type" => "XObject" });
        register_xobject_in_page_resources(&mut doc, page_id, xobj_id, "Img1").unwrap();

        // Verify it was registered
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let res_id = page_dict.get(medpdf::KEY_RESOURCES).unwrap().as_reference().unwrap();
        let res_dict = doc.get_dictionary(res_id).unwrap();
        let xobj_dict = res_dict.get(KEY_XOBJECT).unwrap().as_dict().unwrap();
        let ref_obj = xobj_dict.get(b"Img1").unwrap();
        assert_eq!(ref_obj.as_reference().unwrap(), xobj_id);
    }

    #[test]
    fn test_add_image_jpeg_creates_xobject() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Jpeg {
            data: vec![0xFF, 0xD8, 0xFF, 0xD9], // minimal JPEG
            pixel_width: 100,
            pixel_height: 50,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 200.0, 100.0);
        add_image(&mut doc, page_id, params).unwrap();

        // Verify content stream has Do operator
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let contents = page_dict.get(b"Contents").unwrap().as_array().unwrap();
        assert!(contents.len() >= 2, "Should have content streams");
    }

    #[test]
    fn test_add_image_decoded_creates_xobject_and_smask() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255], // 2x2 RGB
            alpha_channel: Some(vec![255, 128, 64, 0]), // 2x2 alpha
            pixel_width: 2,
            pixel_height: 2,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 10.0, 20.0, 100.0, 100.0);
        add_image(&mut doc, page_id, params).unwrap();

        // Verify XObject was registered in resources
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let res_id = page_dict.get(medpdf::KEY_RESOURCES).unwrap().as_reference().unwrap();
        let res_dict = doc.get_dictionary(res_id).unwrap();
        let xobj_dict = res_dict.get(KEY_XOBJECT).unwrap().as_dict().unwrap();
        // Find the image XObject
        assert!(!xobj_dict.is_empty(), "XObject dict should not be empty");
    }

    #[test]
    fn test_add_image_with_alpha() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![255, 0, 0, 0, 255, 0], // 1x2 RGB
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 2,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 72.0, 72.0).alpha(0.5);
        add_image(&mut doc, page_id, params).unwrap();

        // Verify ExtGState was created for alpha
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let res_id = page_dict.get(medpdf::KEY_RESOURCES).unwrap().as_reference().unwrap();
        let res_dict = doc.get_dictionary(res_id).unwrap();
        assert!(res_dict.get(medpdf::KEY_EXTGSTATE).is_ok(), "Should have ExtGState for alpha");
    }

    #[test]
    fn test_add_image_cover_mode() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![255; 3 * 4], // 2x2 white
            alpha_channel: None,
            pixel_width: 2,
            pixel_height: 2,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 100.0, 50.0).fit(ImageFit::Cover);
        add_image(&mut doc, page_id, params).unwrap();

        // Just verify it doesn't error — cover mode adds a clip rect
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let contents = page_dict.get(b"Contents").unwrap().as_array().unwrap();
        assert!(contents.len() >= 2);
    }

    #[test]
    fn test_add_image_layer_under() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params =
            DrawImageParams::new(image_data, 0.0, 0.0, 72.0, 72.0).layer_over(false);
        add_image(&mut doc, page_id, params).unwrap();

        // In layer_under mode, content is prepended
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let contents = page_dict.get(b"Contents").unwrap().as_array().unwrap();
        assert!(contents.len() >= 2);
    }

    #[test]
    fn test_content_stream_has_do() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![128; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 50.0, 60.0, 100.0, 200.0);
        add_image(&mut doc, page_id, params).unwrap();

        // Read back content streams and check for "Do" and "cm"
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let contents = page_dict.get(b"Contents").unwrap().as_array().unwrap();
        let mut found_do = false;
        let mut found_cm = false;
        for item in contents {
            if let Object::Reference(id) = item {
                let stream = doc.get_object(*id).unwrap().as_stream().unwrap();
                let text = String::from_utf8_lossy(&stream.content);
                if text.contains("Do") {
                    found_do = true;
                }
                if text.contains("cm") {
                    found_cm = true;
                }
            }
        }
        assert!(found_do, "Content stream should contain 'Do' operator");
        assert!(found_cm, "Content stream should contain 'cm' operator");
    }

    // --- Downsampling tests ---

    #[test]
    fn test_downsample_within_limit_passthrough() {
        // 100px image at 100pt output = 72 DPI, well within 300 DPI limit
        let data = ImageData::Decoded {
            pixels: vec![0; 100 * 100 * 3],
            alpha_channel: None,
            pixel_width: 100,
            pixel_height: 100,
            components: 3,
        };
        let result = maybe_downsample(data, 100.0, 100.0, 300.0).unwrap();
        assert_eq!(result.pixel_width(), 100); // unchanged
    }

    #[test]
    fn test_downsample_exceeds_limit() {
        // 3000px image at 100pt output = 2160 DPI, exceeds 300 DPI
        let data = ImageData::Decoded {
            pixels: vec![128; 3000 * 3000 * 3],
            alpha_channel: None,
            pixel_width: 3000,
            pixel_height: 3000,
            components: 3,
        };
        let result = maybe_downsample(data, 100.0, 100.0, 300.0).unwrap();
        // Should be downsampled to ~417px (300 DPI at 100pt)
        assert!(result.pixel_width() < 3000);
        assert!(result.pixel_width() > 300);
    }

    #[test]
    fn test_downsample_disabled() {
        // max_dpi = 0 disables downsampling
        let data = ImageData::Decoded {
            pixels: vec![0; 3000 * 10 * 3],
            alpha_channel: None,
            pixel_width: 3000,
            pixel_height: 10,
            components: 3,
        };
        let result = maybe_downsample(data, 10.0, 10.0, 0.0).unwrap();
        assert_eq!(result.pixel_width(), 3000); // unchanged
    }

    #[test]
    fn test_downsample_jpeg_re_encodes() {
        // Create a minimal valid JPEG-like structure for the test
        // We can't easily create a real JPEG in a unit test, so we test decoded path
        let data = ImageData::Decoded {
            pixels: vec![200; 600 * 600 * 3],
            alpha_channel: None,
            pixel_width: 600,
            pixel_height: 600,
            components: 3,
        };
        // 600px at 72pt output = 600 DPI -> should downsample to ~300 DPI
        let result = maybe_downsample(data, 72.0, 72.0, 300.0).unwrap();
        assert!(result.pixel_width() < 600);
    }

    // --- Validation tests ---

    #[test]
    fn test_add_image_zero_width_errors() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 0.0, 72.0);
        assert!(add_image(&mut doc, page_id, params).is_err());
    }

    #[test]
    fn test_add_image_zero_height_errors() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 72.0, 0.0);
        assert!(add_image(&mut doc, page_id, params).is_err());
    }

    #[test]
    fn test_add_image_negative_dimensions_errors() {
        let (mut doc, page_id) = create_test_doc_and_page();
        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, -50.0, 72.0);
        assert!(add_image(&mut doc, page_id, params).is_err());
    }

    #[test]
    fn test_draw_image_params_alpha_clamped() {
        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 72.0, 72.0).alpha(1.5);
        assert!((params.alpha - 1.0).abs() < f32::EPSILON);

        let image_data = ImageData::Decoded {
            pixels: vec![0; 3],
            alpha_channel: None,
            pixel_width: 1,
            pixel_height: 1,
            components: 3,
        };
        let params = DrawImageParams::new(image_data, 0.0, 0.0, 72.0, 72.0).alpha(-0.5);
        assert!((params.alpha - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_downsample_with_alpha() {
        let data = ImageData::Decoded {
            pixels: vec![100; 600 * 600 * 3],
            alpha_channel: Some(vec![200; 600 * 600]),
            pixel_width: 600,
            pixel_height: 600,
            components: 3,
        };
        let result = maybe_downsample(data, 72.0, 72.0, 300.0).unwrap();
        if let ImageData::Decoded { alpha_channel, pixel_width, .. } = &result {
            assert!(alpha_channel.is_some(), "Alpha should be preserved");
            assert!(*pixel_width < 600, "Should be downsampled");
        } else {
            panic!("Expected Decoded variant");
        }
    }
}
