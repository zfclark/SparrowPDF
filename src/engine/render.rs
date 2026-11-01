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

/// Resolves fonts for text rendering.
pub struct FontSet {
    fonts: Vec<(String, TrueTypeFont)>,
}

impl FontSet {
    /// Locate the first usable system font from a candidate list.
    pub fn system() -> Self {
        const CANDIDATES: &[&str] = &[
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\msyh.ttf",
            r"C:\Windows\Fonts\simsun.ttc",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ];
        let mut fonts = Vec::new();
        for path in CANDIDATES {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            // `.ttc` collections wrap the real font; try the first face.
            let candidates: Vec<Vec<u8>> = if bytes.starts_with(b"ttcf") {
                ttc_offsets(&bytes)
                    .into_iter()
                    .filter_map(|off| bytes.get(off..).map(|s| s.to_vec()))
                    .collect()
            } else {
                vec![bytes]
            };
            for bytes in candidates {
                if let Some(font) = TrueTypeFont::parse(bytes) {
                    fonts.push(((*path).to_string(), font));
                    break;
                }
            }
            if !fonts.is_empty() {
                break;
            }
        }
        Self { fonts }
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    pub fn primary(&self) -> Option<&TrueTypeFont> {
        self.fonts.first().map(|(_, f)| f)
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
        out.push(u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize);
    }
    out
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
    obj.as_float().ok().or_else(|| obj.as_i64().ok().map(|v| v as f32))
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
struct Resources {
    xobjects: BTreeMap<Vec<u8>, ObjectId>,
    extgstates: BTreeMap<Vec<u8>, ObjectId>,
}

fn page_resources(doc: &Document, page_id: ObjectId) -> Resources {
    let mut xobjects = BTreeMap::new();
    let mut extgstates = BTreeMap::new();

    let dict = doc.get_dictionary(page_id).ok();
    let res = dict
        .and_then(|d| d.get(b"Resources").ok())
        .and_then(|o| deref_dict(doc, o));

    if let Some(res) = res {
        if let Some(xo) = res.get(b"XObject").ok().and_then(|o| deref_dict(doc, o)) {
            for (name, value) in xo.iter() {
                if let Ok(id) = value.as_reference() {
                    xobjects.insert(name.clone(), id);
                }
            }
        }
        if let Some(gs) = res
            .get(b"ExtGState")
            .ok()
            .and_then(|o| deref_dict(doc, o))
        {
            for (name, value) in gs.iter() {
                if let Ok(id) = value.as_reference() {
                    extgstates.insert(name.clone(), id);
                }
            }
        }
    }
    Resources {
        xobjects,
        extgstates,
    }
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
                    draw_xobject(
                        doc, pixmap, resources, name, state, fonts, depth,
                    );
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
                    draw_text(
                        pixmap, state, &text, bytes, fonts,
                    );
                }
            }
            "'" => {
                text.line_matrix = text.line_matrix.mul(&Matrix::translate(0.0, -text.leading));
                if let Some(Object::String(bytes, _)) = operands.first() {
                    draw_text(pixmap, state, &text, bytes, fonts);
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
                        draw_text(pixmap, state, &text, bytes, fonts);
                    }
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = operands.first() {
                    for item in items {
                        match item {
                            Object::String(bytes, _) => {
                                draw_text(pixmap, state, &text, bytes, fonts)
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
            let Some(image) = decode_image(doc, stream) else {
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
            execute(
                doc, &ops.operations, pixmap, &mut inner, &mut stack, resources, fonts,
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

/// Paint a PDF string. Bytes are interpreted as Latin-1, the common case for the
/// encodings produced by the tools in this app.
fn draw_text(
    pixmap: &mut Pixmap,
    state: &mut State,
    text: &TextState,
    bytes: &[u8],
    fonts: &FontSet,
) {
    let Some(font) = fonts.primary() else {
        return;
    };
    let size = if text.size.abs() < 0.01 { 12.0 } else { text.size.abs() };
    let upem = font.units_per_em();
    let h_scale = if text.h_scale.abs() < 0.01 { 1.0 } else { text.h_scale };

    // Pen position in text space; the outline is scaled by size/upem.
    let mut pen = text.line_matrix;

    for &byte in bytes {
        let ch = byte as char;
        let gid = font.glyph_for_char(ch);

        if gid != 0 {
            let to_device = state.ctm.mul(&pen).mul(&Matrix::scale(size / upem, size / upem));
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
        }

        // Advance: glyph advance in text space, plus spacing, scaled by Tz.
        let glyph_advance = font.advance(gid) / upem * size;
        let mut step = glyph_advance + text.char_spacing * h_scale;
        if byte == b' ' {
            step += text.word_spacing * h_scale;
        }
        pen = pen.mul(&Matrix::translate(step, 0.0));
    }
}

/// Decode an image XObject into RGBA.
fn decode_image(doc: &Document, stream: &lopdf::Stream) -> Option<ImageRgba> {
    let width = stream.dict.get(b"Width").ok().and_then(number)? as u32;
    let height = stream.dict.get(b"Height").ok().and_then(number)? as u32;
    if width == 0 || height == 0 || width > 8000 || height > 8000 {
        return None;
    }

    let filters: Vec<Vec<u8>> = match stream.dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(arr)) => arr
            .iter()
            .filter_map(|o| o.as_name().ok().map(|n| n.to_vec()))
            .collect(),
        _ => Vec::new(),
    };

    let is_dct = filters.iter().any(|f| f == b"DCTDecode" || f == b"DCT");
    if is_dct {
        let image = image::load_from_memory(&stream.content).ok()?;
        let rgba = image.to_rgba8();
        return Some(ImageRgba {
            width: rgba.width(),
            height: rgba.height(),
            data: rgba.into_raw(),
        });
    }

    let Ok(plain) = stream.decompressed_content() else {
        return None;
    };

    let bits = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(number)
        .unwrap_or(8.0) as u32;
    if bits != 8 {
        return None;
    }

    let components = match stream.dict.get(b"ColorSpace") {
        Ok(Object::Name(name)) => match name.as_slice() {
            b"DeviceGray" | b"G" => 1usize,
            b"DeviceRGB" | b"RGB" => 3,
            b"DeviceCMYK" | b"CMYK" => 4,
            _ => 3,
        },
        _ => 3,
    };

    let needed = width as usize * height as usize * components;
    if plain.len() < needed {
        return None;
    }

    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    match components {
        1 => {
            for &v in &plain[..needed] {
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        4 => {
            for chunk in plain[..needed].chunks_exact(4) {
                let c = chunk[0] as f32 / 255.0;
                let m = chunk[1] as f32 / 255.0;
                let y = chunk[2] as f32 / 255.0;
                let k = chunk[3] as f32 / 255.0;
                data.push(((1.0 - (c + k).min(1.0)) * 255.0) as u8);
                data.push(((1.0 - (m + k).min(1.0)) * 255.0) as u8);
                data.push(((1.0 - (y + k).min(1.0)) * 255.0) as u8);
                data.push(255);
            }
        }
        _ => {
            for chunk in plain[..needed].chunks_exact(3) {
                data.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
    }

    let _ = doc;
    Some(ImageRgba {
        width,
        height,
        data,
    })
}
