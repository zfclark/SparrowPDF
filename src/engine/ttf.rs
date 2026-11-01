//! Minimal TrueType reader used by the pure-Rust page rasterizer.
//!
//! Only what PDF rendering needs is implemented: `head`/`maxp` for metrics,
//! `loca`/`glyf` for outlines, `cmap` (formats 4 and 12) for code -> glyph
//! mapping, and `hmtx` for advances. No new dependency is introduced: these
//! tables are read directly from the bytes.
//!
//! Supported: simple glyphs (quadratic contours) and composite glyphs.

use std::collections::BTreeMap;

/// A flattened outline: closed contours of `(x, y)` in font units.
pub type Contours = Vec<Vec<(f32, f32)>>;

pub struct TrueTypeFont {
    data: Vec<u8>,
    units_per_em: f32,
    /// Offsets into `glyf`, `num_glyphs + 1` entries.
    loca: Vec<u32>,
    glyf: (usize, usize),
    cmap: BTreeMap<u32, u16>,
    advances: Vec<u16>,
    /// Cache of glyph outlines, keyed by glyph id.
    cache: std::cell::RefCell<BTreeMap<u16, Contours>>,
}

impl TrueTypeFont {
    /// Parse a TrueType/OpenType font. Returns `None` for CFF-flavoured fonts
    /// (`OTTO` with no `glyf` table), which this reader cannot outline.
    ///
    /// The app only parses collection faces via [`Self::parse_at`]; kept for
    /// completeness and tests.
    #[allow(dead_code)]
    pub fn parse(data: Vec<u8>) -> Option<Self> {
        Self::parse_at(data, 0)
    }

    /// Parse one face of a TrueType Collection (`ttcf`).
    ///
    /// `dir_base` is the offset of the face's table directory within `data`.
    /// In a collection the table offsets are absolute within the whole file
    /// (they are not relative to the face directory), so `data` must be the
    /// complete file — slicing a face out breaks every table lookup.
    pub fn parse_at(data: Vec<u8>, dir_base: usize) -> Option<Self> {
        if data.len() < dir_base + 12 {
            return None;
        }
        let num_tables = u16::from_be_bytes([data[dir_base + 4], data[dir_base + 5]]) as usize;
        let mut tables: BTreeMap<[u8; 4], (usize, usize)> = BTreeMap::new();
        for i in 0..num_tables {
            let base = dir_base + 12 + i * 16;
            if base + 16 > data.len() {
                return None;
            }
            let tag = [data[base], data[base + 1], data[base + 2], data[base + 3]];
            let offset = u32::from_be_bytes([
                data[base + 8],
                data[base + 9],
                data[base + 10],
                data[base + 11],
            ]) as usize;
            let length = u32::from_be_bytes([
                data[base + 12],
                data[base + 13],
                data[base + 14],
                data[base + 15],
            ]) as usize;
            if offset + length <= data.len() {
                tables.insert(tag, (offset, length));
            }
        }

        let head = *tables.get(b"head")?;
        let maxp = *tables.get(b"maxp")?;
        let glyf = *tables.get(b"glyf")?;
        let loca = *tables.get(b"loca")?;

        let units_per_em =
            u16::from_be_bytes([data[head.0 + 18], data[head.0 + 19]]) as f32;
        let long_loca = i16::from_be_bytes([data[head.0 + 50], data[head.0 + 51]]) == 1;
        let num_glyphs = u16::from_be_bytes([data[maxp.0 + 4], data[maxp.0 + 5]]);

        // hhea governs the hmtx layout; ascent/descent are not needed here
        // because text is positioned from the content stream, not the font.
        let mut num_h_metrics = 0usize;
        if let Some(hhea) = tables.get(b"hhea") {
            num_h_metrics =
                u16::from_be_bytes([data[hhea.0 + 34], data[hhea.0 + 35]]) as usize;
        }

        let mut advances = vec![0u16; num_glyphs as usize];
        if let Some(hmtx) = tables.get(b"hmtx") {
            for gid in 0..num_glyphs as usize {
                let index = gid.min(num_h_metrics.saturating_sub(1));
                let off = hmtx.0 + index * 4;
                if off + 2 <= data.len() {
                    advances[gid] =
                        u16::from_be_bytes([data[off], data[off + 1]]);
                }
            }
        }

        let mut loca_entries = Vec::with_capacity(num_glyphs as usize + 1);
        for i in 0..=num_glyphs as usize {
            let off = loca.0 + if long_loca { i * 4 } else { i * 2 };
            let value = if long_loca {
                if off + 4 <= data.len() {
                    u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                } else {
                    0
                }
            } else if off + 2 <= data.len() {
                u16::from_be_bytes([data[off], data[off + 1]]) as u32
            } else {
                0
            };
            loca_entries.push(value);
        }

        let cmap = tables
            .get(b"cmap")
            .map(|(off, _)| parse_cmap(&data, *off))
            .unwrap_or_default();

        Some(Self {
            data,
            units_per_em: if units_per_em > 0.0 { units_per_em } else { 1000.0 },
            loca: loca_entries,
            glyf,
            cmap,
            advances,
            cache: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// Glyph id for a unicode scalar, or 0 (`.notdef`).
    pub fn glyph_for_char(&self, ch: char) -> u16 {
        self.cmap.get(&(ch as u32)).copied().unwrap_or(0)
    }

    /// Advance width in font units.
    pub fn advance(&self, gid: u16) -> f32 {
        self.advances
            .get(gid as usize)
            .copied()
            .unwrap_or(self.units_per_em as u16) as f32
    }

    /// Outline for `gid` in font units (y axis points up, origin at baseline).
    pub fn outline(&self, gid: u16) -> Contours {
        if let Some(hit) = self.cache.borrow().get(&gid) {
            return hit.clone();
        }
        let mut contours = Vec::new();
        self.build_glyph(gid, &mut contours, 0);
        self.cache.borrow_mut().insert(gid, contours.clone());
        contours
    }

    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        let gid = gid as usize;
        if gid + 1 >= self.loca.len() {
            return None;
        }
        let start = self.loca[gid] as usize;
        let end = self.loca[gid + 1] as usize;
        if start >= end {
            return None;
        }
        let start = self.glyf.0 + start;
        let end = self.glyf.0 + end;
        if end > self.data.len() {
            return None;
        }
        Some((start, end))
    }

    fn build_glyph(&self, gid: u16, out: &mut Contours, depth: u8) {
        if depth > 4 {
            return;
        }
        let Some((start, _end)) = self.glyph_range(gid) else {
            return;
        };
        let d = &self.data;
        let mut p = start;
        if p + 10 > d.len() {
            return;
        }
        let num_contours = i16::from_be_bytes([d[p], d[p + 1]]);
        p += 10;

        if num_contours >= 0 {
            self.build_simple(num_contours as usize, p, out);
        } else {
            self.build_composite(p, out, depth);
        }
    }

    fn build_simple(&self, num_contours: usize, mut p: usize, out: &mut Contours) {
        let d = &self.data;
        if num_contours == 0 {
            return;
        }
        let mut end_points = Vec::with_capacity(num_contours);
        for _ in 0..num_contours {
            if p + 2 > d.len() {
                return;
            }
            end_points.push(u16::from_be_bytes([d[p], d[p + 1]]) as usize);
            p += 2;
        }
        let num_points = match end_points.last() {
            Some(last) => last + 1,
            None => return,
        };
        // Skip the instruction length + instructions.
        if p + 2 > d.len() {
            return;
        }
        let instr_len = u16::from_be_bytes([d[p], d[p + 1]]) as usize;
        p += 2 + instr_len;

        let mut flags: Vec<u8> = Vec::with_capacity(num_points);
        while flags.len() < num_points {
            if p >= d.len() {
                return;
            }
            let flag = d[p];
            p += 1;
            flags.push(flag);
            if flag & 0x08 != 0 {
                if p >= d.len() {
                    return;
                }
                let repeat = d[p];
                p += 1;
                for _ in 0..repeat {
                    flags.push(flag);
                }
            }
        }
        flags.truncate(num_points);

        let mut xs = Vec::with_capacity(num_points);
        let mut x = 0i32;
        for &flag in &flags {
            let delta = if flag & 0x02 != 0 {
                if p >= d.len() {
                    return;
                }
                let v = d[p] as i32;
                p += 1;
                if flag & 0x10 != 0 { v } else { -v }
            } else if flag & 0x10 != 0 {
                0
            } else {
                if p + 2 > d.len() {
                    return;
                }
                let v = i16::from_be_bytes([d[p], d[p + 1]]) as i32;
                p += 2;
                v
            };
            x += delta;
            xs.push(x as f32);
        }

        let mut ys = Vec::with_capacity(num_points);
        let mut y = 0i32;
        for &flag in &flags {
            let delta = if flag & 0x04 != 0 {
                if p >= d.len() {
                    return;
                }
                let v = d[p] as i32;
                p += 1;
                if flag & 0x20 != 0 { v } else { -v }
            } else if flag & 0x20 != 0 {
                0
            } else {
                if p + 2 > d.len() {
                    return;
                }
                let v = i16::from_be_bytes([d[p], d[p + 1]]) as i32;
                p += 2;
                v
            };
            y += delta;
            ys.push(y as f32);
        }

        let mut start = 0usize;
        for &end in &end_points {
            if end >= num_points {
                break;
            }
            let points: Vec<(f32, f32, bool)> = (start..=end)
                .map(|i| (xs[i], ys[i], flags[i] & 0x01 != 0))
                .collect();
            if !points.is_empty() {
                out.push(flatten_quadratic(&points));
            }
            start = end + 1;
        }
    }

    fn build_composite(&self, mut p: usize, out: &mut Contours, depth: u8) {
        let d = &self.data;
        loop {
            if p + 4 > d.len() {
                return;
            }
            let flags = u16::from_be_bytes([d[p], d[p + 1]]);
            let component_gid = u16::from_be_bytes([d[p + 2], d[p + 3]]);
            p += 4;

            let dx = if flags & 0x0001 != 0 {
                // ARG_1_AND_2_ARE_WORDS (the common case)
                if p + 4 > d.len() {
                    return;
                }
                let a = i16::from_be_bytes([d[p], d[p + 1]]) as f32;
                let b = i16::from_be_bytes([d[p + 2], d[p + 3]]) as f32;
                p += 4;
                offset_pair(flags, a, b)
            } else {
                if p + 2 > d.len() {
                    return;
                }
                let a = d[p] as i8 as f32;
                let b = d[p + 1] as i8 as f32;
                p += 2;
                offset_pair(flags, a, b)
            };

            // Skip the transform: only the common 2x2 variants are supported.
            let mut transform = [1.0f32, 0.0, 0.0, 1.0];
            if flags & 0x0008 != 0 {
                if p + 2 > d.len() {
                    return;
                }
                let scale = i16::from_be_bytes([d[p], d[p + 1]]) as f32 / 16384.0;
                transform = [scale, 0.0, 0.0, scale];
                p += 2;
            } else if flags & 0x0040 != 0 {
                if p + 4 > d.len() {
                    return;
                }
                let sx = i16::from_be_bytes([d[p], d[p + 1]]) as f32 / 16384.0;
                let sy = i16::from_be_bytes([d[p + 2], d[p + 3]]) as f32 / 16384.0;
                transform = [sx, 0.0, 0.0, sy];
                p += 4;
            } else if flags & 0x0080 != 0 {
                if p + 8 > d.len() {
                    return;
                }
                let a = i16::from_be_bytes([d[p], d[p + 1]]) as f32 / 16384.0;
                let b = i16::from_be_bytes([d[p + 2], d[p + 3]]) as f32 / 16384.0;
                let c = i16::from_be_bytes([d[p + 4], d[p + 5]]) as f32 / 16384.0;
                let dd = i16::from_be_bytes([d[p + 6], d[p + 7]]) as f32 / 16384.0;
                transform = [a, b, c, dd];
                p += 8;
            }

            let mut child: Contours = Vec::new();
            self.build_glyph(component_gid, &mut child, depth + 1);
            let (tx, ty) = dx;
            for contour in child {
                out.push(
                    contour
                        .into_iter()
                        .map(|(x, y)| {
                            (
                                x * transform[0] + y * transform[2] + tx,
                                x * transform[1] + y * transform[3] + ty,
                            )
                        })
                        .collect(),
                );
            }

            if flags & 0x0020 == 0 {
                break;
            }
        }
    }
}

/// Composite glyph offset: (dx, dy) unless the component uses point matching
/// (`ARG_1_AND_2_ARE_XY_VALUES` clear), in which case we fall back to (0, 0) —
/// point-matched components are rare and only appear in hand-built fonts.
fn offset_pair(flags: u16, a: f32, b: f32) -> (f32, f32) {
    if flags & 0x0002 != 0 {
        (a, b)
    } else {
        (0.0, 0.0)
    }
}

/// Convert a TrueType contour (on-curve / off-curve points) into a polyline.
fn flatten_quadratic(points: &[(f32, f32, bool)]) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(points.len() * 4);

    // Find a starting on-curve point.
    let Some(start_idx) = points.iter().position(|p| p.2) else {
        // All off-curve: use midpoint of the first two.
        let a = points[0];
        let b = points[1 % points.len()];
        out.push(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
        for point in points {
            out.push((point.0, point.1));
        }
        out.push(((points[0].0 + points[1 % points.len()].0) / 2.0,
                  (points[0].1 + points[1 % points.len()].1) / 2.0));
        return out;
    };

    let n = points.len();
    out.push((points[start_idx].0, points[start_idx].1));

    let mut i = 0usize;
    // `current` is always an on-curve point, stored as a plain 2-tuple.
    let mut current: (f32, f32) = (points[start_idx].0, points[start_idx].1);
    while i <= n {
        let point = points[(start_idx + i + 1) % n];
        if point.2 {
            out.push((point.0, point.1));
            current = (point.0, point.1);
            i += 1;
        } else {
            let next = points[(start_idx + i + 2) % n];
            let (cx, cy) = if next.2 {
                (next.0, next.1)
            } else {
                ((point.0 + next.0) / 2.0, (point.1 + next.1) / 2.0)
            };
            push_quad(&mut out, current, (point.0, point.1), (cx, cy));
            current = (cx, cy);
            i += if next.2 { 2 } else { 1 };
        }
        if i >= n {
            break;
        }
    }
    out
}

/// Flatten one quadratic Bezier segment.
fn push_quad(
    out: &mut Vec<(f32, f32)>,
    p0: (f32, f32),
    control: (f32, f32),
    p1: (f32, f32),
) {
    const STEPS: usize = 6;
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.0 + 2.0 * mt * t * control.0 + t * t * p1.0;
        let y = mt * mt * p0.1 + 2.0 * mt * t * control.1 + t * t * p1.1;
        out.push((x, y));
    }
}

fn parse_cmap(data: &[u8], cmap_offset: usize) -> BTreeMap<u32, u16> {
    let mut map = BTreeMap::new();
    if cmap_offset + 4 > data.len() {
        return map;
    }
    let num_tables = u16::from_be_bytes([data[cmap_offset + 2], data[cmap_offset + 3]]) as usize;

    let mut best: Option<(usize, u16)> = None;
    for i in 0..num_tables {
        let rec = cmap_offset + 4 + i * 8;
        if rec + 8 > data.len() {
            break;
        }
        let platform = u16::from_be_bytes([data[rec], data[rec + 1]]);
        let encoding = u16::from_be_bytes([data[rec + 2], data[rec + 3]]);
        let offset = u32::from_be_bytes([
            data[rec + 4],
            data[rec + 5],
            data[rec + 6],
            data[rec + 7],
        ]) as usize;
        let sub = cmap_offset + offset;
        if sub + 2 > data.len() {
            continue;
        }
        let format = u16::from_be_bytes([data[sub], data[sub + 1]]);
        // Prefer a full Unicode BMP/subset then a symbol table.
        let score = match (platform, encoding, format) {
            (3, 10, 12) => 5,
            (0, _, 12) => 4,
            (3, 1, 4) => 3,
            (0, _, 4) => 2,
            (3, 0, 4) => 1,
            _ => 0,
        };
        if score > 0 && best.map(|(_, s)| score > s).unwrap_or(true) {
            best = Some((sub, score));
        }
    }

    if let Some((sub, _)) = best {
        let format = u16::from_be_bytes([data[sub], data[sub + 1]]);
        match format {
            4 => parse_cmap_format4(data, sub, &mut map),
            12 => parse_cmap_format12(data, sub, &mut map),
            _ => {}
        }
    }
    map
}

fn parse_cmap_format4(data: &[u8], sub: usize, map: &mut BTreeMap<u32, u16>) {
    if sub + 14 > data.len() {
        return;
    }
    let seg_count = u16::from_be_bytes([data[sub + 6], data[sub + 7]]) as usize / 2;
    let end_base = sub + 14;
    let start_base = end_base + seg_count * 2 + 2;
    let delta_base = start_base + seg_count * 2;
    let range_base = delta_base + seg_count * 2;

    for seg in 0..seg_count {
        let end = read_u16(data, end_base + seg * 2);
        let start = read_u16(data, start_base + seg * 2);
        let delta = read_i16(data, delta_base + seg * 2);
        let range_offset = read_u16(data, range_base + seg * 2);
        if start > end {
            continue;
        }
        for code in start..=end {
            let gid = if range_offset == 0 {
                (code as i32 + delta as i32) as u16
            } else {
                let addr = range_base + seg * 2 + range_offset as usize + (code - start) as usize * 2;
                let g = read_u16(data, addr);
                if g == 0 {
                    continue;
                }
                (g as i32 + delta as i32) as u16
            };
            if gid != 0 {
                map.insert(code as u32, gid);
            }
            if code == 0xFFFF {
                break;
            }
        }
    }
}

fn parse_cmap_format12(data: &[u8], sub: usize, map: &mut BTreeMap<u32, u16>) {
    if sub + 16 > data.len() {
        return;
    }
    let groups = u32::from_be_bytes([
        data[sub + 12],
        data[sub + 13],
        data[sub + 14],
        data[sub + 15],
    ]) as usize;
    for group in 0..groups.min(4096) {
        let base = sub + 16 + group * 12;
        if base + 12 > data.len() {
            break;
        }
        let start = u32::from_be_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
        let end = u32::from_be_bytes([
            data[base + 4],
            data[base + 5],
            data[base + 6],
            data[base + 7],
        ]);
        let start_gid = u32::from_be_bytes([
            data[base + 8],
            data[base + 9],
            data[base + 10],
            data[base + 11],
        ]);
        for code in start..=end.min(start + 512) {
            let gid = start_gid + (code - start);
            if gid <= u16::MAX as u32 {
                map.insert(code, gid as u16);
            }
        }
    }
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    if at + 2 > data.len() {
        return 0;
    }
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn read_i16(data: &[u8], at: usize) -> i16 {
    if at + 2 > data.len() {
        return 0;
    }
    i16::from_be_bytes([data[at], data[at + 1]])
}
