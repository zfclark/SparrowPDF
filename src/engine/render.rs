//! Pure-Rust PDF page rasterizer.
//!
//! Interprets a page's content stream (via `lopdf`) and paints it with the
//! in-crate software rasterizer in [`super::raster`]. No PDFium, no MuPDF, no
//! C/C++ libraries and no new dependencies are involved.
//!
//! The fidelity boundary is intentional and documented in README: text,
//! vector paths, solid colours, images with DCTDecode/FlateDecode and the
//! standard page transformations are supported; shadings, blend modes,
//! transparency groups, Type3 fonts and soft masks are approximated or
//! skipped.

use std::collections::BTreeMap;

use lopdf::{Document, Object, ObjectId};

use super::raster::{self, ImageRgba, Matrix, Path, Pixmap};
use super::ttf::TrueTypeFont;

/// Rough flavour of a font, used to substitute the closest system face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Sans,
    Serif,
    Mono,
}

struct LoadedFont {
    family: Family,
    font: TrueTypeFont,
}

/// Resolves fonts for text rendering.
///
/// One face per family (sans / serif / mono) is loaded, not just the first
/// match: a document set in Times or Courier kept being drawn with the CJK sans
/// face, and any character that face lacked disappeared. Several faces also give
/// [`Self::resolve`] somewhere to fall back to per glyph.
pub struct FontSet {
    fonts: Vec<LoadedFont>,
}

/// Upper bound on the font bytes kept in memory; a 40 MB CJK collection on
/// macOS would otherwise be followed by two more faces nobody asked for.
const MAX_FONT_BYTES: usize = 56 * 1024 * 1024;

impl FontSet {
    pub fn system() -> Self {
        const SANS: &[&str] = &[
            r"C:/Windows/Fonts/msyh.ttc",
            r"C:/Windows/Fonts/msyh.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            r"C:/Windows/Fonts/segoeui.ttf",
            r"C:/Windows/Fonts/arial.ttf",
        ];
        const SERIF: &[&str] = &[
            r"C:/Windows/Fonts/simsun.ttc",
            r"C:/Windows/Fonts/times.ttf",
            "/System/Library/Fonts/Songti.ttc",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
            r"C:/Windows/Fonts/georgia.ttf",
        ];
        const MONO: &[&str] = &[
            r"C:/Windows/Fonts/consola.ttf",
            r"C:/Windows/Fonts/cour.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ];

        let mut fonts: Vec<LoadedFont> = Vec::new();
        let mut loaded_paths: Vec<String> = Vec::new();
        let mut total_bytes = 0usize;
        for (family, candidates) in [
            (Family::Sans, SANS),
            (Family::Serif, SERIF),
            (Family::Mono, MONO),
        ] {
            for path in candidates {
                if loaded_paths.iter().any(|loaded| loaded == path) {
                    continue;
                }
                if total_bytes >= MAX_FONT_BYTES {
                    break;
                }
                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                total_bytes += bytes.len();
                // `.ttc` collections keep absolute table offsets — parse each
                // face from the full file via `parse_at` (slicing a face out
                // would misalign every table).
                let faces: Vec<(usize, Vec<u8>)> = if bytes.starts_with(b"ttcf") {
                    ttc_offsets(&bytes)
                        .into_iter()
                        .map(|off| (off, bytes.clone()))
                        .collect()
                } else {
                    vec![(0, bytes)]
                };
                let mut parsed = None;
                for (base, data) in faces {
                    if let Some(font) = TrueTypeFont::parse_at(data, base) {
                        parsed = Some(font);
                        break;
                    }
                }
                if let Some(font) = parsed {
                    loaded_paths.push((*path).to_string());
                    fonts.push(LoadedFont { family, font });
                    break;
                }
            }
        }
        Self { fonts }
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    pub fn primary(&self) -> Option<&TrueTypeFont> {
        self.fonts.first().map(|loaded| &loaded.font)
    }

    /// Closest face for a PDF `/BaseFont` name.
    ///
    /// Name matching, not metrics: the point is to stop drawing a Times
    /// document with a CJK gothic face, which changes every glyph's width.
    pub fn pick(&self, base: &str) -> Option<&TrueTypeFont> {
        let lower = base.to_ascii_lowercase();
        let family = if ["times", "serif", "song", "宋", "ming", "georgia", "roman"]
            .iter()
            .any(|key| lower.contains(key))
        {
            Family::Serif
        } else if ["courier", "mono", "consol", "menlo"]
            .iter()
            .any(|key| lower.contains(key))
        {
            Family::Mono
        } else {
            Family::Sans
        };
        self.fonts
            .iter()
            .find(|loaded| loaded.family == family)
            .map(|loaded| &loaded.font)
            .or_else(|| self.primary())
    }

    /// Glyph for `ch`: the preferred face first, then any loaded face.
    ///
    /// The fallback is what keeps a mixed-script run from losing characters —
    /// the reason text used to vanish in the middle of a line.
    pub fn resolve<'a>(
        &'a self,
        preferred: Option<&'a TrueTypeFont>,
        ch: char,
    ) -> Option<(&'a TrueTypeFont, u16)> {
        if let Some(font) = preferred {
            let gid = font.glyph_for_char(ch);
            if gid != 0 {
                return Some((font, gid));
            }
        }
        self.fonts.iter().find_map(|loaded| {
            let gid = loaded.font.glyph_for_char(ch);
            (gid != 0).then_some((&loaded.font, gid))
        })
    }
}

fn ttc_offsets(bytes: &[u8]) -> Vec<usize> {
    if bytes.len() < 12 {
        return Vec::new();
    }
    let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let mut out = Vec::new();
    for i in 0..count.min(4) {
        let at = 12 + i * 4;
        if at + 4 > bytes.len() {
            break;
        }
        out.push(
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize,
        );
    }
    out
}

/// Render the listed 1-based pages of `path` as small thumbnails.
///
/// Opens the document once and scales each page to roughly `height_px`
/// device pixels (handy for preview strips). Returns `(page, pixmap)` pairs
/// in the same order as `pages`.
pub fn render_page_thumbnails(
    path: &std::path::Path,
    password: Option<&str>,
    pages: &[u32],
    height_px: f32,
    fonts: &FontSet,
) -> Result<Vec<(u32, Pixmap)>, String> {
    let doc = crate::engine::pdf_io::open_document(path, password).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(pages.len());
    for &page in pages {
        let page_id = doc
            .get_pages()
            .get(&page)
            .copied()
            .ok_or_else(|| format!("页码 {page} 不存在"))?;
        let (_x0, _y0, _x1, y1) = media_box(&doc, page_id);
        let dpi = ((height_px / y1.abs().max(1.0)) * 72.0)
            .round()
            .clamp(2.0, 72.0) as u32;
        let pixmap = render_page(&doc, page_id, dpi, fonts)?;
        out.push((page, pixmap));
    }
    Ok(out)
}

/// Render one page to an RGBA pixmap at `dpi`.
pub fn render_page(
    doc: &Document,
    page_id: ObjectId,
    dpi: u32,
    fonts: &FontSet,
) -> Result<Pixmap, String> {
    let (x0, y0, x1, y1) = media_box(doc, page_id);
    let page_w = (x1 - x0).abs();
    let page_h = (y1 - y0).abs();
    if page_w < 1.0 || page_h < 1.0 {
        return Err("页面尺寸无效".to_string());
    }

    let rotation = page_rotation(doc, page_id);
    let (vis_w, vis_h) = if matches!(rotation, 90 | 270) {
        (page_h, page_w)
    } else {
        (page_w, page_h)
    };

    let scale = dpi as f32 / 72.0;
    let width = (vis_w * scale).round().max(1.0) as u32;
    let height = (vis_h * scale).round().max(1.0) as u32;

    let mut pixmap = Pixmap::new(width, height, [255, 255, 255]);
    let content = doc
        .get_page_content(page_id)
        .map_err(|e| format!("无法读取页面内容：{e}"))?;
    let ops = lopdf::content::Content::decode(&content)
        .map_err(|e| format!("无法解析页面内容流：{e}"))?;

    // Base transform: user space -> page units -> /Rotate -> device pixels
    // with the y axis flipped so device y grows downward.
    let rotate = match rotation {
        90 => Some(Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, page_w)),
        180 => Some(Matrix::new(-1.0, 0.0, 0.0, -1.0, page_w, page_h)),
        270 => Some(Matrix::new(0.0, 1.0, -1.0, 0.0, page_h, 0.0)),
        _ => None,
    };
    let mut base = Matrix::new(scale, 0.0, 0.0, -scale, 0.0, vis_h * scale);
    if let Some(rotate) = rotate {
        base = base.mul(&rotate);
    }
    base = base.mul(&Matrix::translate(-x0, -y0));

    let mut state = State::new(base);
    let mut stack: Vec<State> = Vec::new();
    let resources = page_resources(doc, page_id);

    execute(
        doc,
        &ops.operations,
        &mut pixmap,
        &mut state,
        &mut stack,
        &resources,
        fonts,
        0,
    );

    Ok(pixmap)
}

fn media_box(doc: &Document, page_id: ObjectId) -> (f32, f32, f32, f32) {
    let mut current = page_id;
    for _ in 0..16 {
        let Ok(dict) = doc.get_dictionary(current) else {
            break;
        };
        if let Ok(Object::Array(arr)) = dict.get(b"MediaBox") {
            if arr.len() >= 4 {
                let nums: Vec<f32> = arr.iter().filter_map(number).collect();
                if nums.len() >= 4 {
                    return (nums[0], nums[1], nums[2], nums[3]);
                }
            }
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => break,
        }
    }
    (0.0, 0.0, 612.0, 792.0)
}

fn page_rotation(doc: &Document, page_id: ObjectId) -> u32 {
    let mut current = page_id;
    for _ in 0..16 {
        let Ok(dict) = doc.get_dictionary(current) else {
            break;
        };
        if let Ok(value) = dict.get(b"Rotate").and_then(Object::as_i64) {
            return ((value % 360 + 360) % 360) as u32;
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => break,
        }
    }
    0
}

fn number(obj: &Object) -> Option<f32> {
    obj.as_float()
        .ok()
        .or_else(|| obj.as_i64().ok().map(|v| v as f32))
}

/// Graphics state for the interpreter.
#[derive(Clone)]
struct State {
    ctm: Matrix,
    fill: [f32; 4],
    stroke: [f32; 4],
    line_width: f32,
    path: Path,
    pending: Option<(f32, f32)>,
}

impl State {
    fn new(ctm: Matrix) -> Self {
        Self {
            ctm,
            fill: [0.0, 0.0, 0.0, 1.0],
            stroke: [0.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            path: Path::new(),
            pending: None,
        }
    }
}

/// Page-level resources, resolved once.
#[derive(Default)]
struct Resources {
    xobjects: BTreeMap<Vec<u8>, ObjectId>,
    extgstates: BTreeMap<Vec<u8>, ObjectId>,
    /// Font resources, keyed by the name used in `Tf`.
    fonts: BTreeMap<Vec<u8>, FontResource>,
}

/// Everything text drawing needs to know about one `/Font` resource.
///
/// The glyphs come from a system font (no font program is executed), but the
/// *characters* must come from the document: a subset or CID font stores codes,
/// not Latin-1 bytes. `/ToUnicode` is the authoritative mapping; without it a
/// simple font falls back to WinAnsi and a composite font gives up.
#[derive(Default)]
struct FontResource {
    /// `/BaseFont`, used to pick a substitute with the right flavour.
    base: String,
    /// Two-byte codes (Type0 / CID-keyed).
    is_cid: bool,
    /// code -> character, parsed from `/ToUnicode`.
    to_unicode: BTreeMap<u32, char>,
    /// Embedded TrueType program (`/FontFile2`), if the file carries one.
    ///
    /// Using it is what makes a subset font readable at all: its codes are
    /// arbitrary indices, and a system font has no idea what they mean.
    embedded: Option<Box<TrueTypeFont>>,
    /// `/CIDToGIDMap`; `None` is `/Identity` (CID == glyph id).
    cid_to_gid: Option<Vec<u16>>,
    /// `/Encoding /Differences`: code -> glyph name.
    differences: BTreeMap<u8, String>,
}

impl FontResource {
    /// Character for one code unit (a CID, or a single byte).
    fn char_for(&self, code: u32) -> Option<char> {
        if let Some(ch) = self.to_unicode.get(&code) {
            return Some(*ch);
        }
        let byte = u8::try_from(code).ok()?;
        self.differences
            .get(&byte)
            .and_then(|name| glyph_name_char(name))
            .or_else(|| win_ansi(byte))
    }

    /// Glyph id inside the embedded program, when the file ships one.
    fn glyph(&self, code: u32, ch: Option<char>) -> Option<u16> {
        let font = self.embedded.as_deref()?;
        if self.is_cid {
            let gid = match &self.cid_to_gid {
                Some(map) => *map.get(code as usize)?,
                None => u16::try_from(code).ok()?,
            };
            return (gid != 0).then_some(gid);
        }
        let gid = font.glyph_for_char(ch?);
        (gid != 0).then_some(gid)
    }
}

fn page_resources(doc: &Document, page_id: ObjectId) -> Resources {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|d| d.get(b"Resources").ok())
        .and_then(|o| deref_dict(doc, o))
        .map(|res| resources_from(doc, res))
        .unwrap_or_default()
}

/// Resolve one `/Resources` dictionary.
///
/// Also used for form XObjects: a form may carry its own dictionary, and its
/// text must be decoded with *that* font table rather than the page's.
fn resources_from(doc: &Document, res: &lopdf::Dictionary) -> Resources {
    let mut xobjects = BTreeMap::new();
    let mut extgstates = BTreeMap::new();
    let mut fonts = BTreeMap::new();

    if let Some(xo) = res.get(b"XObject").ok().and_then(|o| deref_dict(doc, o)) {
        for (name, value) in xo.iter() {
            if let Ok(id) = value.as_reference() {
                xobjects.insert(name.clone(), id);
            }
        }
    }
    if let Some(gs) = res.get(b"ExtGState").ok().and_then(|o| deref_dict(doc, o)) {
        for (name, value) in gs.iter() {
            if let Ok(id) = value.as_reference() {
                extgstates.insert(name.clone(), id);
            }
        }
    }
    if let Some(font_dict) = res.get(b"Font").ok().and_then(|o| deref_dict(doc, o)) {
        for (name, value) in font_dict.iter() {
            if let Ok(id) = value.as_reference() {
                fonts.insert(name.clone(), parse_font(doc, id));
            }
        }
    }
    Resources {
        xobjects,
        extgstates,
        fonts,
    }
}

/// Read the parts of a `/Font` dictionary that affect *which characters* the
/// byte string stands for.
fn parse_font(doc: &Document, id: ObjectId) -> FontResource {
    /// A font program bigger than this is not a subset; keeping it would cost
    /// more memory than substituting a system face.
    const MAX_EMBEDDED_FONT: usize = 32 * 1024 * 1024;

    let Ok(dict) = doc.get_dictionary(id) else {
        return FontResource::default();
    };
    let name_of = |d: &lopdf::Dictionary, key: &[u8]| -> String {
        d.get(key)
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default()
    };
    let is_cid = name_of(dict, b"Subtype") == "Type0";

    // A composite font keeps its descriptor and CID map on the descendant.
    let descendant = if is_cid {
        dict.get(b"DescendantFonts")
            .ok()
            .and_then(|o| match o {
                Object::Array(items) => items.first(),
                other => Some(other),
            })
            .and_then(|o| deref_dict(doc, o))
    } else {
        Some(dict)
    };

    let base = {
        let own = name_of(dict, b"BaseFont");
        if own.is_empty() {
            descendant
                .map(|d| name_of(d, b"BaseFont"))
                .unwrap_or_default()
        } else {
            own
        }
    };

    let to_unicode = dict
        .get(b"ToUnicode")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_stream().ok())
        .and_then(decoded_stream_content)
        .map(|cmap| parse_to_unicode(&cmap))
        .unwrap_or_default();

    let embedded = descendant
        .and_then(|d| d.get(b"FontDescriptor").ok())
        .and_then(|o| deref_dict(doc, o))
        .and_then(|fd| fd.get(b"FontFile2").ok())
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_stream().ok())
        .and_then(decoded_stream_content)
        .filter(|program| program.len() <= MAX_EMBEDDED_FONT)
        .and_then(TrueTypeFont::parse)
        .map(Box::new);

    let cid_to_gid = if is_cid {
        descendant
            .and_then(|d| d.get(b"CIDToGIDMap").ok())
            .and_then(|o| match o {
                // `/Identity` is also the default when the key is absent.
                Object::Name(name) if name == b"Identity" => None,
                Object::Reference(id) => doc
                    .get_object(*id)
                    .ok()
                    .and_then(|o| o.as_stream().ok())
                    .and_then(decoded_stream_content)
                    .map(|bytes| {
                        bytes
                            .chunks_exact(2)
                            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                            .collect()
                    }),
                _ => None,
            })
    } else {
        None
    };

    let differences = if is_cid {
        BTreeMap::new()
    } else {
        dict.get(b"Encoding")
            .ok()
            .and_then(|o| deref_dict(doc, o))
            .and_then(|enc| enc.get(b"Differences").ok())
            .and_then(|o| o.as_array().ok())
            .map(|items| parse_differences(items))
            .unwrap_or_default()
    };

    FontResource {
        base,
        is_cid,
        to_unicode,
        embedded,
        cid_to_gid,
        differences,
    }
}

/// `/Encoding /Differences`: a code number followed by one name per code.
fn parse_differences(items: &[Object]) -> BTreeMap<u8, String> {
    let mut map = BTreeMap::new();
    let mut code = 0u16;
    for item in items {
        match item {
            Object::Integer(_) | Object::Real(_) => {
                if let Some(value) = number(item) {
                    code = value.max(0.0) as u16;
                }
            }
            Object::Name(name) => {
                if let Ok(byte) = u8::try_from(code) {
                    map.insert(byte, String::from_utf8_lossy(name).into_owned());
                }
                code = code.wrapping_add(1);
            }
            _ => {}
        }
    }
    map
}

/// Character named by a `/Differences` glyph name.
///
/// Only the forms that actually appear in text-bearing documents are handled:
/// `uniXXXX`, `uXXXX`, PostScript single-character names, and the common
/// punctuation names. An unknown name falls through to the WinAnsi byte.
fn glyph_name_char(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix("uni") {
        return hex
            .get(..4)
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .and_then(char::from_u32);
    }
    if let Some(hex) = name.strip_prefix('u') {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    let mut chars = name.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        return Some(ch);
    }
    Some(match name {
        "space" => ' ',
        "exclam" => '!',
        "quotedbl" => '"',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "ampersand" => '&',
        "quotesingle" => '\'',
        "parenleft" => '(',
        "parenright" => ')',
        "asterisk" => '*',
        "plus" => '+',
        "comma" => ',',
        "hyphen" | "minus" => '-',
        "period" => '.',
        "slash" => '/',
        "colon" => ':',
        "semicolon" => ';',
        "less" => '<',
        "equal" => '=',
        "greater" => '>',
        "question" => '?',
        "at" => '@',
        "bracketleft" => '[',
        "backslash" => '\\',
        "bracketright" => ']',
        "asciicircum" => '^',
        "underscore" => '_',
        "grave" => '`',
        "braceleft" => '{',
        "bar" => '|',
        "braceright" => '}',
        "asciitilde" => '~',
        "endash" => '\u{2013}',
        "emdash" => '\u{2014}',
        "quoteleft" => '\u{2018}',
        "quoteright" => '\u{2019}',
        "quotedblleft" => '\u{201C}',
        "quotedblright" => '\u{201D}',
        "bullet" => '\u{2022}',
        "ellipsis" => '\u{2026}',
        _ => return None,
    })
}

/// Parse the `bfchar` / `bfrange` sections of a ToUnicode CMap.
///
/// Only the two forms a text-bearing document actually uses are handled; the
/// rest of a CMap (codespace ranges, notdef ranges) does not change how a code
/// maps to a character.
fn parse_to_unicode(cmap: &[u8]) -> BTreeMap<u32, char> {
    let mut map = BTreeMap::new();
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut depth_hex = false;
    for &byte in cmap {
        match byte {
            b'<' if !depth_hex => {
                depth_hex = true;
                current.clear();
            }
            b'>' if depth_hex => {
                depth_hex = false;
                tokens.push(current.clone());
                current.clear();
            }
            b'[' | b']' | b' ' | b'\n' | b'\r' | b'\t' => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ if depth_hex => current.push(byte),
            _ => current.push(byte),
        }
    }

    let value = |token: &[u8]| -> u32 { hex_value(token).unwrap_or(0) };
    let char_of = |token: &[u8]| -> Option<char> {
        let units: Vec<u16> = token
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        text.chars().next()
    };

    // Tokens are in file order; beginbfchar/bfrange markers were consumed as
    // ordinary tokens above, so walk the list and act on the markers.
    let mut index = 0usize;
    while index < tokens.len() {
        let marker = String::from_utf8_lossy(&tokens[index]).into_owned();
        index += 1;
        match marker.as_str() {
            "beginbfchar" => {
                while index + 1 < tokens.len() {
                    if String::from_utf8_lossy(&tokens[index]) == "endbfchar" {
                        index += 1;
                        break;
                    }
                    let code = value(&tokens[index]);
                    if let Some(ch) = char_of(&tokens[index + 1]) {
                        map.insert(code, ch);
                    }
                    index += 2;
                }
            }
            "beginbfrange" => {
                while index + 2 < tokens.len() {
                    if String::from_utf8_lossy(&tokens[index]) == "endbfrange" {
                        index += 1;
                        break;
                    }
                    let low = value(&tokens[index]);
                    let high = value(&tokens[index + 1]);
                    // Array form: <lo> <hi> [ <dst> <dst> … ]
                    if index + 3 < tokens.len()
                        && String::from_utf8_lossy(&tokens[index + 2]) == "["
                    {
                        index += 3;
                        let mut code = low;
                        while index < tokens.len() && String::from_utf8_lossy(&tokens[index]) != "]"
                        {
                            if let Some(ch) = char_of(&tokens[index]) {
                                map.insert(code, ch);
                            }
                            code += 1;
                            index += 1;
                        }
                        index += 1;
                        continue;
                    }
                    let Some(first) = char_of(&tokens[index + 2]) else {
                        index += 3;
                        continue;
                    };
                    let base = first as u32;
                    for offset in 0..=high.saturating_sub(low) {
                        if let Some(ch) = char::from_u32(base + offset) {
                            map.insert(low + offset, ch);
                        }
                    }
                    index += 3;
                }
            }
            _ => {}
        }
    }
    map
}

fn hex_value(token: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    let mut digits = 0;
    for &b in token {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a') as u32 + 10,
            b'A'..=b'F' => (b - b'A') as u32 + 10,
            _ => continue,
        };
        value = value * 16 + digit;
        digits += 1;
    }
    (digits > 0).then_some(value)
}

fn deref_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute(
    doc: &Document,
    ops: &[lopdf::content::Operation],
    pixmap: &mut Pixmap,
    state: &mut State,
    stack: &mut Vec<State>,
    resources: &Resources,
    fonts: &FontSet,
    depth: u8,
) {
    if depth > 8 {
        return;
    }

    let mut text = TextState::default();
    let mut text_stack: Vec<TextState> = Vec::new();

    for op in ops {
        let operands = &op.operands;
        match op.operator.as_str() {
            "q" => {
                stack.push(state.clone());
                text_stack.push(text.clone());
            }
            "Q" => {
                if let Some(previous) = stack.pop() {
                    *state = previous;
                }
                if let Some(previous) = text_stack.pop() {
                    text = previous;
                }
            }
            "cm" => {
                if let Some(m) = matrix_from(operands) {
                    state.ctm = state.ctm.mul(&m);
                }
            }
            "w" => {
                if let Some(v) = operands.first().and_then(number) {
                    state.line_width = v;
                }
            }
            "g" => {
                if let Some(v) = operands.first().and_then(number) {
                    state.fill = [v, v, v, state.fill[3]];
                }
            }
            "G" => {
                if let Some(v) = operands.first().and_then(number) {
                    state.stroke = [v, v, v, state.stroke[3]];
                }
            }
            "rg" => {
                if operands.len() >= 3 {
                    let c: Vec<f32> = operands.iter().filter_map(number).collect();
                    if c.len() >= 3 {
                        state.fill = [c[0], c[1], c[2], state.fill[3]];
                    }
                }
            }
            "RG" => {
                if operands.len() >= 3 {
                    let c: Vec<f32> = operands.iter().filter_map(number).collect();
                    if c.len() >= 3 {
                        state.stroke = [c[0], c[1], c[2], state.stroke[3]];
                    }
                }
            }
            "k" => {
                if operands.len() >= 4 {
                    let c: Vec<f32> = operands.iter().filter_map(number).collect();
                    if c.len() >= 4 {
                        state.fill = cmyk_to_rgba(c[0], c[1], c[2], c[3], state.fill[3]);
                    }
                }
            }
            "K" => {
                if operands.len() >= 4 {
                    let c: Vec<f32> = operands.iter().filter_map(number).collect();
                    if c.len() >= 4 {
                        state.stroke = cmyk_to_rgba(c[0], c[1], c[2], c[3], state.stroke[3]);
                    }
                }
            }
            "m" => {
                if operands.len() >= 2 {
                    if let (Some(x), Some(y)) = (number(&operands[0]), number(&operands[1])) {
                        let p = state.ctm.apply((x, y));
                        state.path.move_to(p.0, p.1);
                        state.pending = Some((x, y));
                    }
                }
            }
            "l" => {
                if operands.len() >= 2 {
                    if let (Some(x), Some(y)) = (number(&operands[0]), number(&operands[1])) {
                        let p = state.ctm.apply((x, y));
                        state.path.line_to(p.0, p.1);
                        state.pending = Some((x, y));
                    }
                }
            }
            "c" => {
                if operands.len() >= 6 {
                    let n: Vec<f32> = operands.iter().filter_map(number).collect();
                    if n.len() >= 6 {
                        let c1 = state.ctm.apply((n[0], n[1]));
                        let c2 = state.ctm.apply((n[2], n[3]));
                        let end = state.ctm.apply((n[4], n[5]));
                        state.path.cubic_to(c1, c2, end);
                        state.pending = Some((n[4], n[5]));
                    }
                }
            }
            "v" | "y" => {
                if operands.len() >= 4 {
                    let n: Vec<f32> = operands.iter().filter_map(number).collect();
                    if n.len() >= 4 {
                        let start = state.pending.unwrap_or((n[0], n[1]));
                        let c1 = state.ctm.apply((n[0], n[1]));
                        let end = state.ctm.apply((n[2], n[3]));
                        state.path.cubic_to(c1, c1, end);
                        let _ = start;
                        state.pending = Some((n[2], n[3]));
                    }
                }
            }
            "h" => state.path.close(),
            "re" => {
                if operands.len() >= 4 {
                    let n: Vec<f32> = operands.iter().filter_map(number).collect();
                    if n.len() >= 4 {
                        let rect = Path {
                            contours: vec![vec![
                                state.ctm.apply((n[0], n[1])),
                                state.ctm.apply((n[0] + n[2], n[1])),
                                state.ctm.apply((n[0] + n[2], n[1] + n[3])),
                                state.ctm.apply((n[0], n[1] + n[3])),
                            ]],
                            closed: vec![true],
                        };
                        state.path.contours.extend(rect.contours);
                        state.path.closed.extend(rect.closed);
                    }
                }
            }
            "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                let even_odd = op.operator.ends_with('*');
                if even_odd {
                    raster::fill_even_odd(pixmap, &state.path, state.fill);
                } else {
                    raster::fill(pixmap, &state.path, state.fill);
                }
                if op.operator.starts_with('B') || op.operator.starts_with('b') {
                    let width = state.line_width * state.ctm.stroke_scale();
                    raster::stroke(pixmap, &state.path, state.stroke, width);
                }
                if op.operator.starts_with('b') {
                    state.path.close();
                }
                state.path = Path::new();
            }
            "S" | "s" => {
                if op.operator == "s" {
                    state.path.close();
                }
                let width = state.line_width * state.ctm.stroke_scale();
                raster::stroke(pixmap, &state.path, state.stroke, width.max(0.4));
                state.path = Path::new();
            }
            "n" => state.path = Path::new(),
            "gs" => {
                if let Some(Object::Name(name)) = operands.first() {
                    apply_extgstate(doc, pixmap, resources, name, state);
                }
            }
            "Do" => {
                if let Some(Object::Name(name)) = operands.first() {
                    draw_xobject(doc, pixmap, resources, name, state, fonts, depth);
                }
            }
            "BT" => {
                text = TextState::default();
                text.line_matrix = Matrix::IDENTITY;
            }
            "ET" => {}
            "Tf" => {
                if operands.len() >= 2 {
                    if let Object::Name(name) = &operands[0] {
                        text.font = Some(name.clone());
                    }
                    if let Some(size) = number(&operands[1]) {
                        text.size = size;
                    }
                }
            }
            "Tm" => {
                if let Some(m) = matrix_from(operands) {
                    text.line_matrix = m;
                }
            }
            "Td" => {
                if operands.len() >= 2 {
                    if let (Some(tx), Some(ty)) = (number(&operands[0]), number(&operands[1])) {
                        text.line_matrix = text.line_matrix.mul(&Matrix::translate(tx, ty));
                    }
                }
            }
            "TD" => {
                if operands.len() >= 2 {
                    if let (Some(tx), Some(ty)) = (number(&operands[0]), number(&operands[1])) {
                        text.line_matrix = text.line_matrix.mul(&Matrix::translate(tx, ty));
                        text.leading = -ty;
                    }
                }
            }
            "T*" => {
                text.line_matrix = text.line_matrix.mul(&Matrix::translate(0.0, -text.leading));
            }
            "TL" => {
                if let Some(v) = operands.first().and_then(number) {
                    text.leading = v;
                }
            }
            "Tc" => {
                if let Some(v) = operands.first().and_then(number) {
                    text.char_spacing = v;
                }
            }
            "Tw" => {
                if let Some(v) = operands.first().and_then(number) {
                    text.word_spacing = v;
                }
            }
            "Tz" => {
                if let Some(v) = operands.first().and_then(number) {
                    text.h_scale = v / 100.0;
                }
            }
            "Tj" => {
                if let Some(Object::String(bytes, _)) = operands.first() {
                    draw_text(pixmap, state, &text, bytes, fonts, resources);
                }
            }
            "'" => {
                text.line_matrix = text.line_matrix.mul(&Matrix::translate(0.0, -text.leading));
                if let Some(Object::String(bytes, _)) = operands.first() {
                    draw_text(pixmap, state, &text, bytes, fonts, resources);
                }
            }
            "\"" => {
                if operands.len() >= 3 {
                    if let (Some(aw), Some(ac)) = (number(&operands[0]), number(&operands[1])) {
                        text.word_spacing = aw;
                        text.char_spacing = ac;
                    }
                    text.line_matrix = text.line_matrix.mul(&Matrix::translate(0.0, -text.leading));
                    if let Object::String(bytes, _) = &operands[2] {
                        draw_text(pixmap, state, &text, bytes, fonts, resources);
                    }
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = operands.first() {
                    for item in items {
                        match item {
                            Object::String(bytes, _) => {
                                draw_text(pixmap, state, &text, bytes, fonts, resources)
                            }
                            other => {
                                if let Some(k) = number(other) {
                                    // Negative k in TJ moves text forward.
                                    let tx = -k / 1000.0 * text.size * text.h_scale;
                                    text.line_matrix =
                                        text.line_matrix.mul(&Matrix::translate(tx, 0.0));
                                }
                            }
                        }
                    }
                }
            }
            "BI" => {
                // Inline images are skipped: they are rare in generated PDFs and
                // parsing them requires the matching ID/EI scan.
            }
            _ => {}
        }
    }
}

fn cmyk_to_rgba(c: f32, m: f32, y: f32, k: f32, alpha: f32) -> [f32; 4] {
    [
        1.0 - (c + k).min(1.0),
        1.0 - (m + k).min(1.0),
        1.0 - (y + k).min(1.0),
        alpha,
    ]
}

fn matrix_from(operands: &[Object]) -> Option<Matrix> {
    if operands.len() < 6 {
        return None;
    }
    let n: Vec<f32> = operands.iter().filter_map(number).collect();
    if n.len() < 6 {
        return None;
    }
    Some(Matrix::new(n[0], n[1], n[2], n[3], n[4], n[5]))
}

fn apply_extgstate(
    doc: &Document,
    _pixmap: &mut Pixmap,
    resources: &Resources,
    name: &[u8],
    state: &mut State,
) {
    let Some(id) = resources.extgstates.get(name) else {
        return;
    };
    let Ok(dict) = doc.get_dictionary(*id) else {
        return;
    };
    if let Some(ca) = dict.get(b"ca").ok().and_then(number) {
        state.fill[3] = ca.clamp(0.0, 1.0);
    }
    if let Some(ca) = dict.get(b"CA").ok().and_then(number) {
        state.stroke[3] = ca.clamp(0.0, 1.0);
    }
}

/// Draw an image XObject or recurse into a form XObject.
fn draw_xobject(
    doc: &Document,
    pixmap: &mut Pixmap,
    resources: &Resources,
    name: &[u8],
    state: &mut State,
    fonts: &FontSet,
    depth: u8,
) {
    let Some(id) = resources.xobjects.get(name) else {
        return;
    };
    let Ok(Object::Stream(stream)) = doc.get_object(*id) else {
        return;
    };
    let subtype = stream.dict.get(b"Subtype").and_then(Object::as_name).ok();

    match subtype {
        Some(b"Image") => {
            let Some(image) = decode_image(doc, stream, state.fill) else {
                return;
            };
            // The unit square maps through the CTM.
            let corners = [
                state.ctm.apply((0.0, 0.0)),
                state.ctm.apply((1.0, 0.0)),
                state.ctm.apply((1.0, 1.0)),
                state.ctm.apply((0.0, 1.0)),
            ];
            let alpha = state.fill[3];
            raster::draw_image(pixmap, &image, corners, alpha);
        }
        Some(b"Form") => {
            let Ok(content) = stream.decompressed_content() else {
                return;
            };
            let Ok(ops) = lopdf::content::Content::decode(&content) else {
                return;
            };
            let matrix = stream.dict.get(b"Matrix").ok().and_then(|o| {
                o.as_array().ok().and_then(|arr| {
                    let n: Vec<f32> = arr.iter().filter_map(number).collect();
                    if n.len() >= 6 {
                        Some(Matrix::new(n[0], n[1], n[2], n[3], n[4], n[5]))
                    } else {
                        None
                    }
                })
            });

            let mut inner = state.clone();
            if let Some(m) = matrix {
                inner.ctm = state.ctm.mul(&m);
            }
            inner.path = Path::new();
            let mut stack: Vec<State> = Vec::new();
            // A form may carry its own dictionary; its text must be decoded with
            // that font table, otherwise the codes resolve against the page's
            // fonts (or nothing) and the run disappears.
            let inner_resources = stream
                .dict
                .get(b"Resources")
                .ok()
                .and_then(|o| deref_dict(doc, o))
                .map(|dict| resources_from(doc, dict));
            execute(
                doc,
                &ops.operations,
                pixmap,
                &mut inner,
                &mut stack,
                inner_resources.as_ref().unwrap_or(resources),
                fonts,
                depth + 1,
            );
        }
        _ => {}
    }
}

/// Text state for the interpreter.
#[derive(Clone)]
struct TextState {
    font: Option<Vec<u8>>,
    size: f32,
    line_matrix: Matrix,
    leading: f32,
    char_spacing: f32,
    word_spacing: f32,
    h_scale: f32,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            size: 0.0,
            line_matrix: Matrix::IDENTITY,
            leading: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
        }
    }
}

/// Paint a PDF string.
///
/// The bytes are *codes*, not characters: a CID font stores two-byte glyph ids
/// and a subset font stores whatever the producer chose. `/ToUnicode` turns them
/// back into characters; without it a simple font is read as WinAnsi (the
/// encoding of every standard-14 font) and a composite font draws nothing rather
/// than random Latin letters. Glyphs come from a substituted system font, with a
/// per-character fallback so a document mixing scripts does not lose characters.
fn draw_text(
    pixmap: &mut Pixmap,
    state: &mut State,
    text: &TextState,
    bytes: &[u8],
    fonts: &FontSet,
    resources: &Resources,
) {
    let resource = text
        .font
        .as_ref()
        .and_then(|name| resources.fonts.get(name));
    // A document that carries its own glyph program is drawn from it; the
    // system font is only a stand-in for fonts the file does not embed.
    let embedded = resource.and_then(|r| r.embedded.as_deref());
    if embedded.is_none() && fonts.is_empty() {
        return;
    }
    let preferred = fonts.pick(resource.map(|r| r.base.as_str()).unwrap_or_default());
    let size = if text.size.abs() < 0.01 {
        12.0
    } else {
        text.size.abs()
    };
    let h_scale = if text.h_scale.abs() < 0.01 {
        1.0
    } else {
        text.h_scale
    };

    // (character, embedded glyph, is a single-byte space) per code unit.
    let decoded: Vec<(Option<char>, Option<u16>, bool)> = match resource {
        Some(font) if font.is_cid => bytes
            .chunks(2)
            .map(|pair| {
                let code = u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]) as u32;
                let ch = font.char_for(code);
                (ch, font.glyph(code, ch), false)
            })
            .collect(),
        Some(font) => bytes
            .iter()
            .map(|&byte| {
                let code = byte as u32;
                let ch = font.char_for(code);
                (ch, font.glyph(code, ch), byte == b' ')
            })
            .collect(),
        None => bytes
            .iter()
            .map(|&byte| (win_ansi(byte), None, byte == b' '))
            .collect(),
    };

    // Pen position in text space; the outline is scaled by size/upem.
    let mut pen = text.line_matrix;

    for (ch, embedded_gid, is_space) in decoded {
        let mut step = size * 0.5;
        // The embedded program wins when it has the glyph; otherwise a system
        // face that does. A subset without the character falls back per glyph.
        let drawn = match (embedded, embedded_gid) {
            (Some(font), Some(gid)) => Some((font, gid)),
            _ => ch.and_then(|ch| fonts.resolve(preferred, ch)),
        };
        if let Some((font, gid)) = drawn {
            let upem = font.units_per_em();
            let to_device = state
                .ctm
                .mul(&pen)
                .mul(&Matrix::scale(size / upem, size / upem));
            let mut path = Path::new();
            for contour in font.outline(gid) {
                if contour.len() < 2 {
                    continue;
                }
                let first = to_device.apply(contour[0]);
                path.move_to(first.0, first.1);
                for point in contour.iter().skip(1) {
                    let p = to_device.apply(*point);
                    path.line_to(p.0, p.1);
                }
                path.close();
            }
            raster::fill(pixmap, &path, state.fill);
            step = font.advance(gid) / upem * size;
        }

        // Advance: glyph advance in text space, plus spacing, scaled by Tz.
        step += text.char_spacing * h_scale;
        if is_space {
            step += text.word_spacing * h_scale;
        }
        pen = pen.mul(&Matrix::translate(step, 0.0));
    }
}

/// WinAnsi byte -> character.
///
/// Latin-1 for 0x20-0x7E and 0xA0-0xFF; the 0x80-0x9F band is where WinAnsi
/// differs from Latin-1 (typographic quotes, dashes, the euro sign). Reading
/// that band as Latin-1 is what turned curly quotes into control characters —
/// and, once a font had no glyph for them, into missing text.
fn win_ansi(byte: u8) -> Option<char> {
    const SPECIALS: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    match byte {
        0x20..=0x7E | 0xA0..=0xFF => Some(byte as char),
        0x80..=0x9F => {
            let ch = SPECIALS[(byte - 0x80) as usize];
            (ch != '\u{FFFD}').then_some(ch)
        }
        _ => None,
    }
}

/// Decode an image XObject into RGBA.
///
/// The supported encodings cover what actually shows up in text documents:
/// DCTDecode (JPEG, passed through), FlateDecode and LZWDecode (optionally with
/// a PNG or TIFF predictor), CCITTFaxDecode (G3 1D/2D and G4) and JBIG2. A
/// stencil mask (`/ImageMask true`) is painted with the current fill colour
/// instead of black.
fn decode_image(doc: &Document, stream: &lopdf::Stream, fill: [f32; 4]) -> Option<ImageRgba> {
    let width = stream.dict.get(b"Width").ok().and_then(number)? as u32;
    let height = stream.dict.get(b"Height").ok().and_then(number)? as u32;
    if width == 0 || height == 0 || width > 8000 || height > 8000 {
        return None;
    }
    let filters = filter_names(stream);
    let image_mask = stream
        .dict
        .get(b"ImageMask")
        .ok()
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);
    let paint = Paint {
        mask: image_mask,
        fill,
        inverted: decode_inverted(stream),
    };

    // JPEG: hand the original bytes to the decoder, no re-encoding anywhere.
    if filters.iter().any(|f| f == b"DCTDecode" || f == b"DCT") {
        let image = image::load_from_memory(&stream.content).ok()?;
        let rgba = image.to_rgba8();
        return Some(ImageRgba {
            width: rgba.width(),
            height: rgba.height(),
            data: rgba.into_raw(),
        });
    }

    // Bilevel codecs hand back a 1-bit raster, not samples: they get their own
    // path so a 4000-px-wide scan is not mistaken for a 4000-byte row.
    if let Some(index) = filters
        .iter()
        .position(|f| f == b"CCITTFaxDecode" || f == b"CCF")
    {
        let payload = payload_before(doc, stream, &filters, index)?;
        let (gray, cols, rows) = ccitt_gray(doc, stream, index, &payload, height)?;
        return Some(bilevel_rgba(&gray, cols, rows, paint));
    }
    if let Some(index) = filters
        .iter()
        .position(|f| f == b"JBIG2Decode" || f == b"JBIG2")
    {
        let payload = payload_before(doc, stream, &filters, index)?;
        let (gray, cols, rows) = jbig2_gray(doc, stream, index, &payload)?;
        return Some(bilevel_rgba(&gray, cols, rows, paint));
    }

    // Byte-oriented chain: every stage yields the input of the next one.
    let mut data = stream.content.clone();
    for (index, filter) in filters.iter().enumerate() {
        match filter.as_slice() {
            b"FlateDecode" | b"Fl" => data = inflate(&data)?,
            b"LZWDecode" | b"LZW" => {
                let early = parms(doc, stream, index)
                    .and_then(|p| p.get(b"EarlyChange").ok())
                    .and_then(|o| o.as_bool().ok())
                    .unwrap_or(true);
                data = lzw_decode(&data, early)?;
            }
            _ => return None,
        }
        if let Some(params) = parms(doc, stream, index) {
            let predictor = params
                .get(b"Predictor")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(1);
            if predictor > 1 {
                let colors = params
                    .get(b"Colors")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(1)
                    .max(1) as usize;
                let bpc = params
                    .get(b"BitsPerComponent")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(8)
                    .clamp(1, 16) as usize;
                let columns = params
                    .get(b"Columns")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(1)
                    .max(1) as usize;
                data = undo_predictor(&data, predictor, colors, bpc, columns)?;
            }
        }
    }

    samples_to_rgba(stream, &data, width, height, paint)
}

/// How a decoded raster should become pixels.
#[derive(Clone, Copy)]
struct Paint {
    /// `/ImageMask true`: painted pixels take `fill`, the rest stay transparent.
    mask: bool,
    /// Current non-stroking colour, used by stencil masks.
    fill: [f32; 4],
    /// `/Decode` lists the high value first, i.e. black and white are swapped.
    inverted: bool,
}

/// Bytes of an auxiliary stream (ToUnicode CMap, JBIG2 globals).
///
/// `decompressed_content` refuses a stream without `/Filter`, so an
/// uncompressed CMap — common in hand-written and older producers — has to be
/// taken verbatim.
fn decoded_stream_content(stream: &lopdf::Stream) -> Option<Vec<u8>> {
    if stream.dict.get(b"Filter").is_ok() {
        stream.decompressed_content().ok()
    } else {
        Some(stream.content.clone())
    }
}

/// Filter names in decoding order (a single `/Filter /Name` is a one-element
/// chain).
fn filter_names(stream: &lopdf::Stream) -> Vec<Vec<u8>> {
    match stream.dict.get(b"Filter") {
        Ok(Object::Name(name)) => vec![name.clone()],
        Ok(Object::Array(items)) => items
            .iter()
            .filter_map(|o| o.as_name().ok().map(|n| n.to_vec()))
            .collect(),
        _ => Vec::new(),
    }
}

/// `/DecodeParms` entry that belongs to `filters[index]`.
fn parms<'a>(
    doc: &'a Document,
    stream: &'a lopdf::Stream,
    index: usize,
) -> Option<&'a lopdf::Dictionary> {
    let value = stream.dict.get(b"DecodeParms").ok()?;
    match value {
        // One entry per filter, aligned by position.
        Object::Array(items) => match items.get(index)? {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => doc.get_dictionary(*id).ok(),
            _ => None,
        },
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        _ => None,
    }
}

/// Bytes a bilevel codec sees: the output of every filter before it.
///
/// A CCITT or JBIG2 stage is occasionally wrapped in Flate; feeding the raw
/// stream content would decode noise.
fn payload_before(
    doc: &Document,
    stream: &lopdf::Stream,
    filters: &[Vec<u8>],
    index: usize,
) -> Option<Vec<u8>> {
    let mut data = stream.content.clone();
    for (position, filter) in filters.iter().enumerate().take(index) {
        data = match filter.as_slice() {
            b"FlateDecode" | b"Fl" => inflate(&data)?,
            b"LZWDecode" | b"LZW" => {
                let early = parms(doc, stream, position)
                    .and_then(|p| p.get(b"EarlyChange").ok())
                    .and_then(|o| o.as_bool().ok())
                    .unwrap_or(true);
                lzw_decode(&data, early)?
            }
            _ => return None,
        };
    }
    Some(data)
}

/// `/Decode` starts at the high value, i.e. black and white are swapped.
fn decode_inverted(stream: &lopdf::Stream) -> bool {
    let Ok(Object::Array(values)) = stream.dict.get(b"Decode") else {
        return false;
    };
    let first = values.first().and_then(number);
    let second = values.get(1).and_then(number);
    matches!((first, second), (Some(a), Some(b)) if a > b)
}

/// Decode a CCITT G3/G4 stream into a 1-byte-per-pixel raster.
fn ccitt_gray(
    doc: &Document,
    stream: &lopdf::Stream,
    index: usize,
    payload: &[u8],
    image_height: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};

    let params = parms(doc, stream, index);
    let get_int = |key: &[u8], default: i64| -> i64 {
        params
            .and_then(|p| p.get(key).ok())
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(default)
    };
    let get_bool = |key: &[u8], default: bool| -> bool {
        params
            .and_then(|p| p.get(key).ok())
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(default)
    };

    let k = get_int(b"K", 0);
    let settings = DecodeSettings {
        // 1728 is the fax default; PDF producers usually write it explicitly.
        columns: get_int(b"Columns", 1728).clamp(1, 20_000) as u32,
        // A `/Rows` smaller than the image height is wrong more often than the
        // image height is, so the larger value wins (same as Chromium).
        rows: (get_int(b"Rows", 0).max(i64::from(image_height))).clamp(1, 20_000) as u32,
        end_of_block: get_bool(b"EndOfBlock", true),
        end_of_line: get_bool(b"EndOfLine", false),
        rows_are_byte_aligned: get_bool(b"EncodedByteAlign", false),
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k == 0 {
            EncodingMode::Group3_1D
        } else {
            EncodingMode::Group3_2D { k: k as u32 }
        },
        // Deliberately *not* driven by `/BlackIs1`: that flag describes how the
        // producer packed the samples, while the fax runs already carry the
        // visual colour. This adapter emits a luminance raster (0 = dark), so
        // fax-black is dark whichever way the samples were packed.
        invert_black: false,
    };

    struct Gray {
        out: Vec<u8>,
        index: usize,
        rows: u32,
    }
    impl Decoder for Gray {
        fn push_pixels(&mut self, white: bool, count: u32) {
            let end = (self.index + count as usize).min(self.out.len());
            if !white {
                self.out[self.index..end].fill(0);
            }
            self.index = end;
        }
        fn next_line(&mut self) {
            self.rows += 1;
        }
    }

    let total = settings.columns as usize * settings.rows as usize;
    let mut decoder = Gray {
        out: vec![0xFF; total],
        index: 0,
        rows: 0,
    };
    let mut context = DecoderContext::new(settings);
    let result = hayro_ccitt::decode(payload, &mut decoder, &mut context);
    // Partial data is still worth showing: a truncated fax is a page, not an
    // error message, and the rows before the truncation are correct.
    if result.is_err() && decoder.rows == 0 {
        return None;
    }
    Some((decoder.out, settings.columns, settings.rows))
}

/// Decode a JBIG2 stream (with optional shared globals) into a raster.
fn jbig2_gray(
    doc: &Document,
    stream: &lopdf::Stream,
    index: usize,
    payload: &[u8],
) -> Option<(Vec<u8>, u32, u32)> {
    let globals = parms(doc, stream, index)
        .and_then(|p| p.get(b"JBIG2Globals").ok())
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_stream().ok())
        .and_then(decoded_stream_content);

    let image = hayro_jbig2::Image::new_embedded(payload, globals.as_deref()).ok()?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > 20_000 || height > 20_000 {
        return None;
    }

    struct Gray {
        out: Vec<u8>,
        index: usize,
    }
    impl hayro_jbig2::Decoder for Gray {
        fn push_pixel(&mut self, black: bool) {
            if self.index < self.out.len() && black {
                self.out[self.index] = 0;
            }
            self.index += 1;
        }
        fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
            let count = chunk_count as usize * 8;
            let end = (self.index + count).min(self.out.len());
            if black {
                self.out[self.index..end].fill(0);
            }
            self.index = end;
        }
        fn next_line(&mut self) {}
    }

    let mut decoder = Gray {
        out: vec![0xFF; width as usize * height as usize],
        index: 0,
    };
    image.decode(&mut decoder).ok()?;
    Some((decoder.out, width, height))
}

/// Turn a 1-byte-per-pixel black/white raster into RGBA.
///
/// For a stencil mask the "painted" pixels take the current fill colour and the
/// rest stay transparent, which is how scanned logos and signatures are stored.
fn bilevel_rgba(gray: &[u8], width: u32, height: u32, paint: Paint) -> ImageRgba {
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    let ink = [
        (paint.fill[0] * 255.0).clamp(0.0, 255.0) as u8,
        (paint.fill[1] * 255.0).clamp(0.0, 255.0) as u8,
        (paint.fill[2] * 255.0).clamp(0.0, 255.0) as u8,
    ];
    for &value in gray.iter().take(width as usize * height as usize) {
        let mut dark = value < 128;
        if paint.inverted {
            dark = !dark;
        }
        if paint.mask {
            if dark {
                data.extend_from_slice(&[ink[0], ink[1], ink[2], 255]);
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        } else if dark {
            data.extend_from_slice(&[0, 0, 0, 255]);
        } else {
            data.extend_from_slice(&[255, 255, 255, 255]);
        }
    }
    ImageRgba {
        width,
        height,
        data,
    }
}

/// Unpack 8-bit and sub-byte samples, then map them through the colour space.
fn samples_to_rgba(
    stream: &lopdf::Stream,
    plain: &[u8],
    width: u32,
    height: u32,
    paint: Paint,
) -> Option<ImageRgba> {
    let bits = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(number)
        .unwrap_or(8.0)
        .clamp(1.0, 16.0) as u32;
    let components = match stream.dict.get(b"ColorSpace") {
        Ok(Object::Name(name)) => match name.as_slice() {
            b"DeviceGray" | b"G" => 1usize,
            b"DeviceCMYK" | b"CMYK" => 4,
            _ => 3,
        },
        _ => 3,
    };
    if paint.mask && bits != 1 {
        return None;
    }
    let samples = unpack_samples(plain, width, height, components, bits)?;
    if paint.mask {
        // A mask has one component and paints where the sample is "on".
        let gray: Vec<u8> = samples.iter().map(|&s| 255 - s).collect();
        return Some(bilevel_rgba(&gray, width, height, paint));
    }

    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    match components {
        1 => {
            for &value in &samples {
                let mut value = value;
                if paint.inverted {
                    value = 255 - value;
                }
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        4 => {
            for chunk in samples.chunks_exact(4) {
                let mut c = [chunk[0], chunk[1], chunk[2], chunk[3]];
                if paint.inverted {
                    // Inverting CMYK means inverting all four inks.
                    c = [255 - c[0], 255 - c[1], 255 - c[2], 255 - c[3]];
                }
                let k = c[3] as f32 / 255.0;
                for &ink in &c[..3] {
                    let v = ink as f32 / 255.0;
                    data.push(((1.0 - (v + k).min(1.0)) * 255.0) as u8);
                }
                data.push(255);
            }
        }
        _ => {
            for chunk in samples.chunks_exact(3) {
                data.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
    }
    Some(ImageRgba {
        width,
        height,
        data,
    })
}

/// Expand samples of `bits` width into one byte per sample (high bit first).
fn unpack_samples(
    plain: &[u8],
    width: u32,
    height: u32,
    components: usize,
    bits: u32,
) -> Option<Vec<u8>> {
    let count = width as usize * height as usize * components;
    if bits == 8 {
        if plain.len() < count {
            return None;
        }
        return Some(plain[..count].to_vec());
    }
    if bits > 8 {
        // 16-bit samples are extremely rare in page content; scaling them down
        // by taking the high byte keeps the image recognisable.
        if plain.len() < count * 2 {
            return None;
        }
        return Some(plain.chunks_exact(2).take(count).map(|c| c[0]).collect());
    }

    let per_byte = (8 / bits) as usize;
    let max = ((1u32 << bits) - 1) as f32;
    let mut out = Vec::with_capacity(count);
    'outer: for &byte in plain {
        for slot in 0..per_byte {
            let shift = 8 - bits * (slot as u32 + 1);
            let raw = (byte >> shift) & ((1u32 << bits) - 1) as u8;
            out.push((raw as f32 / max * 255.0).round() as u8);
            if out.len() == count {
                break 'outer;
            }
        }
    }
    (out.len() == count).then_some(out)
}

/// Undo a PNG (`10..=15`) or TIFF (`2`) predictor, row by row.
fn undo_predictor(
    data: &[u8],
    predictor: i64,
    colors: usize,
    bits: usize,
    columns: usize,
) -> Option<Vec<u8>> {
    let bpp = (colors * bits).div_ceil(8);
    let row_len = (columns * colors * bits).div_ceil(8);
    if bpp == 0 || row_len == 0 {
        return None;
    }

    if predictor == 2 {
        // TIFF: each sample is stored as the delta to the previous one.
        if bits != 8 {
            return None;
        }
        let mut out = data.to_vec();
        for row in out.chunks_mut(row_len) {
            for index in bpp..row.len() {
                row[index] = row[index].wrapping_add(row[index - bpp]);
            }
        }
        return Some(out);
    }

    // PNG predictors carry a filter-type byte in front of every row.
    let stride = row_len + 1;
    let rows = data.len() / stride;
    let mut out = Vec::with_capacity(rows * row_len);
    let mut previous = vec![0u8; row_len];
    for row_index in 0..rows {
        let start = row_index * stride;
        let filter = data[start];
        let row = &data[start + 1..start + stride];
        let mut current = vec![0u8; row_len];
        for index in 0..row_len {
            let raw = row[index] as i32;
            let left = if index >= bpp {
                current[index - bpp] as i32
            } else {
                0
            };
            let up = previous[index] as i32;
            let up_left = if index >= bpp {
                previous[index - bpp] as i32
            } else {
                0
            };
            let value = match filter {
                0 => raw,
                1 => raw + left,
                2 => raw + up,
                3 => raw + (left + up) / 2,
                4 => {
                    // Paeth
                    let p = left + up - up_left;
                    let pa = (p - left).abs();
                    let pb = (p - up).abs();
                    let pc = (p - up_left).abs();
                    if pa <= pb && pa <= pc {
                        raw + left
                    } else if pb <= pc {
                        raw + up
                    } else {
                        raw + up_left
                    }
                }
                _ => return None,
            };
            current[index] = value as u8;
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Some(out)
}

/// zlib/inflate decode.
fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// PDF LZW decode (`/EarlyChange` chooses between the two code-width rules).
fn lzw_decode(data: &[u8], early_change: bool) -> Option<Vec<u8>> {
    use weezl::{decode::Decoder, BitOrder};
    let mut decoder = if early_change {
        // PDF's default is the TIFF variant, which widens one symbol sooner.
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    decoder.decode(data).ok()
}
