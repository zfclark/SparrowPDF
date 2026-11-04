//! A small software rasterizer: RGBA pixmap, polygon fill, and stroke.
//!
//! Written from scratch because the dependency list is locked and no drawing
//! crate is available. Anti-aliasing comes from 4x4 supersampling inside the
//! scanline coverage calculation.

#[derive(Debug, Clone)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major.
    pub data: Vec<u8>,
}

impl Pixmap {
    pub fn new(width: u32, height: u32, background: [u8; 3]) -> Self {
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize) * (height as usize) {
            data.extend_from_slice(&[background[0], background[1], background[2], 255]);
        }
        Self {
            width,
            height,
            data,
        }
    }

    #[inline]
    fn index(&self, x: u32, y: u32) -> usize {
        ((y as usize) * (self.width as usize) + (x as usize)) * 4
    }

    /// Source-over blend of one pixel.
    #[inline]
    pub fn blend(&mut self, x: u32, y: u32, color: [f32; 4], coverage: f32) {
        if x >= self.width || y >= self.height || coverage <= 0.0 {
            return;
        }
        let a = (color[3] * coverage).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = self.index(x, y);
        let dst = [
            self.data[i] as f32 / 255.0,
            self.data[i + 1] as f32 / 255.0,
            self.data[i + 2] as f32 / 255.0,
        ];
        for c in 0..3 {
            let mixed = color[c] * a + dst[c] * (1.0 - a);
            self.data[i + c] = (mixed * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        self.data[i + 3] = 255;
    }

    /// Encode as PNG. The `image` crate's PNG codec is used for the container
    /// only — the pixels come from this rasterizer.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let rgba = image::RgbaImage::from_raw(self.width, self.height, self.data.clone())
            .ok_or_else(|| "像素缓冲尺寸不符".to_string())?;
        let mut buf: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// Encode as JPEG at quality 85.
    pub fn to_jpeg(&self) -> Result<Vec<u8>, String> {
        // JPEG has no alpha channel; composite onto white first.
        let mut rgb = Vec::with_capacity((self.width as usize) * (self.height as usize) * 3);
        for chunk in self.data.chunks_exact(4) {
            let a = chunk[3] as f32 / 255.0;
            for &channel in &chunk[..3] {
                let v = channel as f32 * a + 255.0 * (1.0 - a);
                rgb.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        let buffer = image::RgbImage::from_raw(self.width, self.height, rgb)
            .ok_or_else(|| "像素缓冲尺寸不符".to_string())?;
        let mut out: Vec<u8> = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
        encoder
            .encode_image(&image::DynamicImage::ImageRgb8(buffer))
            .map_err(|e| e.to_string())?;
        Ok(out)
    }
}

/// A device-space point.
pub type Point = (f32, f32);

/// A flattened path in device space.
#[derive(Debug, Clone, Default)]
pub struct Path {
    pub contours: Vec<Vec<Point>>,
    pub closed: Vec<bool>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.contours.push(vec![(x, y)]);
        self.closed.push(false);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        if let Some(last) = self.contours.last_mut() {
            last.push((x, y));
        } else {
            self.move_to(x, y);
        }
    }

    pub fn close(&mut self) {
        if let Some(flag) = self.closed.last_mut() {
            *flag = true;
        }
    }

    pub fn cubic_to(&mut self, c1: Point, c2: Point, end: Point) {
        let Some(start) = self.contours.last().and_then(|c| c.last().copied()) else {
            self.move_to(end.0, end.1);
            return;
        };
        const STEPS: usize = 12;
        for step in 1..=STEPS {
            let t = step as f32 / STEPS as f32;
            let mt = 1.0 - t;
            let x = mt * mt * mt * start.0
                + 3.0 * mt * mt * t * c1.0
                + 3.0 * mt * t * t * c2.0
                + t * t * t * end.0;
            let y = mt * mt * mt * start.1
                + 3.0 * mt * mt * t * c1.1
                + 3.0 * mt * t * t * c2.1
                + t * t * t * end.1;
            self.line_to(x, y);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|c| c.len() < 2)
    }
}

/// Affine transform, PDF order `[a b c d e f]`.
#[derive(Debug, Clone, Copy)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn new(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// `self * other` — applies `other` first, then `self` (PDF convention).
    ///
    /// `apply` uses the column-vector form `x' = a·x + c·y + e`,
    /// `y' = b·x + d·y + f`, so the product must be the plain matrix product of
    /// the two column-vector matrices. Mixing the two conventions here made
    /// every composed transform (`cm` before `Do`/`Tj`, page `/Rotate`,
    /// form-XObject `/Matrix`) collapse: the translation term of `other` was
    /// scaled by `self`'s translation instead of its rotation.
    pub fn mul(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn apply(&self, (x, y): Point) -> Point {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Extra scale factor applied to stroke widths.
    pub fn stroke_scale(&self) -> f32 {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        ((sx * sx + sy * sy) / 2.0).sqrt()
    }
}

/// Fill a path using the non-zero winding rule.
pub fn fill(pixmap: &mut Pixmap, path: &Path, color: [f32; 4]) {
    fill_rule(pixmap, path, color, false);
}

/// Fill a path using the even-odd rule.
pub fn fill_even_odd(pixmap: &mut Pixmap, path: &Path, color: [f32; 4]) {
    fill_rule(pixmap, path, color, true);
}

fn fill_rule(pixmap: &mut Pixmap, path: &Path, color: [f32; 4], even_odd: bool) {
    if path.is_empty() || color[3] <= 0.0 {
        return;
    }

    // Gather edges once. A fill closes every subpath implicitly, whether or
    // not the path was built with `close`: skipping the wrap-around edge for
    // closed contours dropped exactly one edge per shape, which left
    // axis-aligned rectangles (a PDF `re`) with a single crossing edge and
    // therefore no fill at all.
    let mut edges: Vec<(f32, f32, f32, f32, i32)> = Vec::new();
    for contour in path.contours.iter() {
        if contour.len() < 2 {
            continue;
        }
        let mut points = contour.clone();
        points.push(contour[0]);
        for pair in points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if (y0 - y1).abs() < 1e-9 {
                continue;
            }
            let dir = if y0 < y1 { 1 } else { -1 };
            edges.push((x0, y0, x1, y1, dir));
        }
    }
    if edges.is_empty() {
        return;
    }

    const SAMPLES: i32 = 4;
    let min_y = edges
        .iter()
        .map(|e| e.1.min(e.3))
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i64;
    let max_y = edges
        .iter()
        .map(|e| e.1.max(e.3))
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(pixmap.height as f32) as i64;

    // Coverage of the row being scanned, accumulated over the vertical
    // sub-samples and blended exactly once per pixel. Blending per sub-sample
    // would compose the same colour four times (1 - (1 - 1/4)^4 ≈ 0.68), so
    // solid areas came out ~30% too light.
    let mut coverage = vec![0.0f32; pixmap.width as usize];

    for y in min_y..max_y {
        if y < 0 || y >= pixmap.height as i64 {
            continue;
        }
        coverage.iter_mut().for_each(|c| *c = 0.0);
        for s in 0..SAMPLES {
            let sy = y as f32 + (s as f32 + 0.5) / SAMPLES as f32;
            let spans = if even_odd {
                even_odd_spans(&edges, sy)
            } else {
                winding_spans(&edges, sy)
            };

            for (xa, xb) in spans {
                if xb <= xa {
                    continue;
                }
                let start = xa.max(0.0).floor() as i64;
                let end = xb.min(pixmap.width as f32).ceil() as i64;
                for x in start..end {
                    if x < 0 || x >= pixmap.width as i64 {
                        continue;
                    }
                    // Fraction of this pixel covered horizontally by the span,
                    // times the vertical sub-sample weight.
                    let px0 = x as f32;
                    let px1 = px0 + 1.0;
                    let overlap = xb.min(px1) - xa.max(px0);
                    if overlap <= 0.0 {
                        continue;
                    }
                    coverage[x as usize] += overlap.clamp(0.0, 1.0) / SAMPLES as f32;
                }
            }
        }
        for (x, cov) in coverage.iter().enumerate() {
            if *cov > 0.0 {
                pixmap.blend(x as u32, y as u32, color, cov.min(1.0));
            }
        }
    }
}

/// Even-odd spans for one scanline.
fn even_odd_spans(edges: &[(f32, f32, f32, f32, i32)], sy: f32) -> Vec<(f32, f32)> {
    let mut hits: Vec<f32> = Vec::new();
    for &(x0, y0, x1, y1, _) in edges {
        let (top, bottom) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        if sy < top || sy >= bottom {
            continue;
        }
        let t = (sy - y0) / (y1 - y0);
        hits.push(x0 + t * (x1 - x0));
    }
    if hits.len() < 2 {
        return Vec::new();
    }
    hits.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    hits.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

/// Non-zero winding spans for one scanline.
fn winding_spans(edges: &[(f32, f32, f32, f32, i32)], sy: f32) -> Vec<(f32, f32)> {
    let mut hits: Vec<(f32, i32)> = Vec::new();
    for &(x0, y0, x1, y1, dir) in edges {
        let (top, bottom) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        if sy < top || sy >= bottom {
            continue;
        }
        let t = (sy - y0) / (y1 - y0);
        hits.push((x0 + t * (x1 - x0), dir));
    }
    if hits.len() < 2 {
        return Vec::new();
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut spans = Vec::new();
    let mut winding = 0i32;
    let mut start = 0.0f32;
    for (x, dir) in hits {
        let was_inside = winding != 0;
        winding += dir;
        let is_inside = winding != 0;
        if !was_inside && is_inside {
            start = x;
        } else if was_inside && !is_inside {
            spans.push((start, x));
        }
    }
    spans
}

/// Stroke a path by expanding each segment into a quad.
pub fn stroke(pixmap: &mut Pixmap, path: &Path, color: [f32; 4], width: f32) {
    if width <= 0.0 || color[3] <= 0.0 {
        return;
    }
    let half = width / 2.0;
    let mut out = Path::new();

    for (index, contour) in path.contours.iter().enumerate() {
        if contour.len() < 2 {
            continue;
        }
        let closed = path.closed.get(index).copied().unwrap_or(false);
        let segments: Vec<(Point, Point)> = contour
            .windows(2)
            .map(|w| (w[0], w[1]))
            .chain(if closed {
                Some((contour[contour.len() - 1], contour[0]))
            } else {
                None
            })
            .collect();

        for (a, b) in segments {
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            let len = (dx * dx + dy * dy).sqrt();
            if len <= 1e-6 {
                continue;
            }
            let nx = -dy / len * half;
            let ny = dx / len * half;
            out.move_to(a.0 + nx, a.1 + ny);
            out.line_to(b.0 + nx, b.1 + ny);
            out.line_to(b.0 - nx, b.1 - ny);
            out.line_to(a.0 - nx, a.1 - ny);
            out.close();
            // Round-ish joints: a small square at each vertex.
            out.move_to(a.0 + half, a.1 + half);
            out.line_to(a.0 + half, a.1 - half);
            out.line_to(a.0 - half, a.1 - half);
            out.line_to(a.0 - half, a.1 + half);
            out.close();
        }
    }
    fill(pixmap, &out, color);
}

/// Draw an axis-aligned image into the pixmap, sampling nearest-neighbour.
pub fn draw_image(pixmap: &mut Pixmap, image: &ImageRgba, corners: [Point; 4], alpha: f32) {
    if image.width == 0 || image.height == 0 || alpha <= 0.0 {
        return;
    }
    // Axis-aligned fast path only; rotated images are skipped rather than wrong.
    let xs = [corners[0].0, corners[1].0, corners[2].0, corners[3].0];
    let ys = [corners[0].1, corners[1].1, corners[2].1, corners[3].1];
    let min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
        return;
    }

    let dw = max_x - min_x;
    let dh = max_y - min_y;
    if dw.abs() < 0.5 || dh.abs() < 0.5 {
        return;
    }

    let x_start = min_x.max(0.0).floor() as i64;
    let x_end = max_x.min(pixmap.width as f32).ceil() as i64;
    let y_start = min_y.max(0.0).floor() as i64;
    let y_end = max_y.min(pixmap.height as f32).ceil() as i64;

    for py in y_start..y_end {
        for px in x_start..x_end {
            if px < 0 || py < 0 || px >= pixmap.width as i64 || py >= pixmap.height as i64 {
                continue;
            }
            let u = ((px as f32 + 0.5 - min_x) / dw).clamp(0.0, 0.999);
            let v = ((py as f32 + 0.5 - min_y) / dh).clamp(0.0, 0.999);
            let sx = (u * image.width as f32) as u32;
            let sy = (v * image.height as f32) as u32;
            let idx = ((sy as usize) * image.width as usize + sx as usize) * 4;
            if idx + 3 >= image.data.len() {
                continue;
            }
            let a = image.data[idx + 3] as f32 / 255.0 * alpha;
            let color = [
                image.data[idx] as f32 / 255.0,
                image.data[idx + 1] as f32 / 255.0,
                image.data[idx + 2] as f32 / 255.0,
                a,
            ];
            pixmap.blend(px as u32, py as u32, color, 1.0);
        }
    }
}

/// Decoded RGBA image ready for drawing.
pub struct ImageRgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::Matrix;

    /// `mul` must compose in the same convention `apply` evaluates, otherwise
    /// any `cm` / `/Rotate` / form matrix lands the geometry somewhere else
    /// (images ended up a whole page-height below the page and rendered blank).
    #[test]
    fn matrix_mul_matches_apply_order() {
        let base = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 595.28);
        let cm = Matrix::new(824.0, 0.0, 0.0, 595.28, 8.0, 0.0);

        // `self * other` applies `other` first.
        let composed = base.mul(&cm);
        let p = composed.apply((0.0, 0.0));
        assert!((p.0 - 8.0).abs() < 0.01, "x = {}", p.0);
        assert!((p.1 - 595.28).abs() < 0.01, "y = {}", p.1);

        let p = composed.apply((1.0, 1.0));
        assert!((p.0 - 832.0).abs() < 0.01, "x = {}", p.0);
        assert!(p.1.abs() < 0.01, "y = {}", p.1);
    }

    #[test]
    fn matrix_mul_rotation_maps_the_page_box() {
        // A 841.89 x 595.28 page with /Rotate 90: the visual box is
        // 595.28 x 841.89, which is what `render_page` feeds the base matrix.
        let base = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 841.89);
        let rotate = Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, 841.89);
        let composed = base.mul(&rotate);
        // User-space origin is the bottom-right corner of the visual box.
        let p = composed.apply((0.0, 0.0));
        assert!(
            (p.0 - 0.0).abs() < 0.01 && (p.1 - 0.0).abs() < 0.01,
            "{p:?}"
        );
        // (841.89, 0) runs up the left edge of the rotated page.
        let p = composed.apply((841.89, 0.0));
        assert!(
            (p.0 - 0.0).abs() < 0.05 && (p.1 - 841.89).abs() < 0.05,
            "{p:?}"
        );
    }
}
