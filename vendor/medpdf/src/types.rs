/// Rich types for the medpdf crate: color, alignment, font properties, and AddTextParams.
/// An RGBA color with components in [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Alpha component (0.0 = fully transparent, 1.0 = fully opaque).
    /// Supported by `add_text_params()` via an ExtGState dictionary with `ca`/`CA`
    /// parameters. When alpha is 1.0, no ExtGState is emitted (zero overhead).
    pub a: f32,
}

impl PdfColor {
    pub const BLACK: PdfColor = PdfColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: PdfColor = PdfColor {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const RED: PdfColor = PdfColor {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    pub fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    pub fn from_argb8(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// Returns a copy with all components clamped to [0.0, 1.0].
    pub fn clamped(self) -> Self {
        Self {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
            a: self.a.clamp(0.0, 1.0),
        }
    }
}

impl Default for PdfColor {
    fn default() -> Self {
        Self::BLACK
    }
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Vertical text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    /// Typographic ascent — includes headroom for diacriticals (Å, Ö).
    Top,
    /// Cap height — tight alignment to top of capital letters (H, T).
    CapTop,
    Center,
    #[default]
    Baseline,
    /// Typographic descent line — the "expected" descent, some deep descenders may extend below.
    DescentBottom,
    /// Font bounding box bottom — guaranteed to clear every glyph in the font.
    Bottom,
}

/// Font weight (numeric, matching CSS/OpenType conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: FontWeight = FontWeight(100);
    pub const EXTRA_LIGHT: FontWeight = FontWeight(200);
    pub const LIGHT: FontWeight = FontWeight(300);
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const MEDIUM: FontWeight = FontWeight(500);
    pub const SEMI_BOLD: FontWeight = FontWeight(600);
    pub const BOLD: FontWeight = FontWeight(700);
    pub const EXTRA_BOLD: FontWeight = FontWeight(800);
    pub const BLACK: FontWeight = FontWeight(900);
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// Font style (normal, italic, or oblique).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique(f32),
}

/// Parameters for adding text to a PDF page with full control over rendering.
#[derive(Debug, Clone)]
pub struct AddTextParams {
    pub text: String,
    pub font_data: crate::font_data::FontData,
    pub font_name: String,
    pub font_size: f32,
    pub x: f32,
    pub y: f32,
    pub color: PdfColor,
    pub rotation: f32,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub layer_over: bool,
    pub strikeout: bool,
    pub underline: bool,
}

impl AddTextParams {
    pub fn new(text: impl Into<String>, font_data: crate::font_data::FontData, font_name: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_data,
            font_name: font_name.into(),
            font_size: 12.0,
            x: 0.0,
            y: 0.0,
            color: PdfColor::BLACK,
            rotation: 0.0,
            h_align: HAlign::Left,
            v_align: VAlign::Baseline,
            layer_over: true,
            strikeout: false,
            underline: false,
        }
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn position(mut self, x: f32, y: f32) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    pub fn color(mut self, color: PdfColor) -> Self {
        self.color = color;
        self
    }

    pub fn rotation(mut self, degrees: f32) -> Self {
        self.rotation = degrees;
        self
    }

    pub fn h_align(mut self, align: HAlign) -> Self {
        self.h_align = align;
        self
    }

    pub fn v_align(mut self, align: VAlign) -> Self {
        self.v_align = align;
        self
    }

    pub fn layer_over(mut self, over: bool) -> Self {
        self.layer_over = over;
        self
    }

    pub fn strikeout(mut self, strikeout: bool) -> Self {
        self.strikeout = strikeout;
        self
    }

    pub fn underline(mut self, underline: bool) -> Self {
        self.underline = underline;
        self
    }
}

/// Parameters for drawing a filled rectangle on a PDF page.
#[derive(Debug, Clone)]
pub struct DrawRectParams {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: PdfColor,
    pub layer_over: bool,
}

impl DrawRectParams {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            color: PdfColor::BLACK,
            layer_over: true,
        }
    }

    pub fn color(mut self, color: PdfColor) -> Self {
        self.color = color;
        self
    }

    pub fn layer_over(mut self, over: bool) -> Self {
        self.layer_over = over;
        self
    }
}

/// Parameters for drawing a stroked line on a PDF page.
#[derive(Debug, Clone)]
pub struct DrawLineParams {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub line_width: f32,
    pub color: PdfColor,
    pub layer_over: bool,
}

impl DrawLineParams {
    pub fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x1,
            y1,
            x2,
            y2,
            line_width: 1.0,
            color: PdfColor::BLACK,
            layer_over: true,
        }
    }

    pub fn line_width(mut self, width: f32) -> Self {
        self.line_width = width;
        self
    }

    pub fn color(mut self, color: PdfColor) -> Self {
        self.color = color;
        self
    }

    pub fn layer_over(mut self, over: bool) -> Self {
        self.layer_over = over;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_data::FontData;

    #[test]
    fn test_pdf_color_rgb() {
        let c = PdfColor::rgb(0.5, 0.6, 0.7);
        assert!((c.r - 0.5).abs() < f32::EPSILON);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_from_rgb8() {
        let c = PdfColor::from_rgb8(255, 0, 128);
        assert!((c.r - 1.0).abs() < f32::EPSILON);
        assert!((c.g - 0.0).abs() < f32::EPSILON);
        assert!((c.b - 128.0 / 255.0).abs() < 0.001);
    }

    #[test]
    fn test_pdf_color_constants() {
        assert_eq!(PdfColor::BLACK, PdfColor::rgb(0.0, 0.0, 0.0));
        assert_eq!(PdfColor::WHITE, PdfColor::rgb(1.0, 1.0, 1.0));
        assert_eq!(PdfColor::RED, PdfColor::rgb(1.0, 0.0, 0.0));
    }

    #[test]
    fn test_defaults() {
        assert_eq!(PdfColor::default(), PdfColor::BLACK);
        assert_eq!(HAlign::default(), HAlign::Left);
        assert_eq!(VAlign::default(), VAlign::Baseline);
        assert_eq!(FontWeight::default(), FontWeight::NORMAL);
        assert_eq!(FontStyle::default(), FontStyle::Normal);
    }

    #[test]
    fn test_add_text_params_builder() {
        let params = AddTextParams::new("Hello", FontData::Embedded(std::sync::Arc::new(vec![1, 2, 3])), "TestFont")
            .font_size(24.0)
            .position(100.0, 200.0)
            .color(PdfColor::RED)
            .rotation(45.0)
            .h_align(HAlign::Center)
            .v_align(VAlign::Top)
            .layer_over(false);

        assert_eq!(params.text, "Hello");
        assert!((params.font_size - 24.0).abs() < f32::EPSILON);
        assert!((params.x - 100.0).abs() < f32::EPSILON);
        assert!((params.y - 200.0).abs() < f32::EPSILON);
        assert_eq!(params.color, PdfColor::RED);
        assert!((params.rotation - 45.0).abs() < f32::EPSILON);
        assert_eq!(params.h_align, HAlign::Center);
        assert_eq!(params.v_align, VAlign::Top);
        assert!(!params.layer_over);
    }

    #[test]
    fn test_font_weight_ordering() {
        assert!(FontWeight::THIN < FontWeight::NORMAL);
        assert!(FontWeight::NORMAL < FontWeight::BOLD);
        assert!(FontWeight::BOLD < FontWeight::BLACK);
    }

    // --- PdfColor::from_argb8 ---

    #[test]
    fn test_pdf_color_from_argb8_full_alpha() {
        let c = PdfColor::from_argb8(255, 255, 0, 128);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
        assert!((c.r - 1.0).abs() < f32::EPSILON);
        assert!((c.g - 0.0).abs() < f32::EPSILON);
        assert!((c.b - 128.0 / 255.0).abs() < 0.001);
    }

    #[test]
    fn test_pdf_color_from_argb8_half_alpha() {
        let c = PdfColor::from_argb8(128, 0, 255, 0);
        assert!((c.a - 128.0 / 255.0).abs() < 0.001);
        assert!((c.r - 0.0).abs() < f32::EPSILON);
        assert!((c.g - 1.0).abs() < f32::EPSILON);
        assert!((c.b - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_from_argb8_zero_alpha() {
        let c = PdfColor::from_argb8(0, 100, 100, 100);
        assert!((c.a - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_from_argb8_all_zeros() {
        let c = PdfColor::from_argb8(0, 0, 0, 0);
        assert!((c.a - 0.0).abs() < f32::EPSILON);
        assert!((c.r - 0.0).abs() < f32::EPSILON);
        assert!((c.g - 0.0).abs() < f32::EPSILON);
        assert!((c.b - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_from_argb8_all_max() {
        let c = PdfColor::from_argb8(255, 255, 255, 255);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
        assert!((c.r - 1.0).abs() < f32::EPSILON);
        assert!((c.g - 1.0).abs() < f32::EPSILON);
        assert!((c.b - 1.0).abs() < f32::EPSILON);
    }

    // --- PdfColor::clamped ---

    #[test]
    fn test_pdf_color_clamped_in_range() {
        let c = PdfColor::rgba(0.5, 0.3, 0.7, 0.9);
        let clamped = c.clamped();
        assert_eq!(c, clamped);
    }

    #[test]
    fn test_pdf_color_clamped_above_max() {
        let c = PdfColor::rgba(1.5, 2.0, 0.5, 3.0);
        let clamped = c.clamped();
        assert!((clamped.r - 1.0).abs() < f32::EPSILON);
        assert!((clamped.g - 1.0).abs() < f32::EPSILON);
        assert!((clamped.b - 0.5).abs() < f32::EPSILON);
        assert!((clamped.a - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_clamped_below_min() {
        let c = PdfColor::rgba(-0.5, 0.0, -1.0, -0.1);
        let clamped = c.clamped();
        assert!((clamped.r - 0.0).abs() < f32::EPSILON);
        assert!((clamped.g - 0.0).abs() < f32::EPSILON);
        assert!((clamped.b - 0.0).abs() < f32::EPSILON);
        assert!((clamped.a - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pdf_color_clamped_boundary_values() {
        let c = PdfColor::rgba(0.0, 1.0, 0.0, 1.0);
        let clamped = c.clamped();
        assert_eq!(c, clamped);
    }

    #[test]
    fn test_pdf_color_clamped_does_not_mutate() {
        let c = PdfColor::rgba(2.0, -1.0, 0.5, 0.5);
        let _clamped = c.clamped();
        // Original should be unchanged
        assert!((c.r - 2.0).abs() < f32::EPSILON);
        assert!((c.g - (-1.0)).abs() < f32::EPSILON);
    }

    // --- PdfColor::from_rgb8 boundary values ---

    #[test]
    fn test_pdf_color_from_rgb8_all_zeros() {
        let c = PdfColor::from_rgb8(0, 0, 0);
        assert!((c.r - 0.0).abs() < f32::EPSILON);
        assert!((c.g - 0.0).abs() < f32::EPSILON);
        assert!((c.b - 0.0).abs() < f32::EPSILON);
        assert!((c.a - 1.0).abs() < f32::EPSILON); // alpha defaults to 1.0
    }

    #[test]
    fn test_pdf_color_from_rgb8_all_max() {
        let c = PdfColor::from_rgb8(255, 255, 255);
        assert!((c.r - 1.0).abs() < f32::EPSILON);
        assert!((c.g - 1.0).abs() < f32::EPSILON);
        assert!((c.b - 1.0).abs() < f32::EPSILON);
    }

    // --- PdfColor::rgba ---

    #[test]
    fn test_pdf_color_rgba() {
        let c = PdfColor::rgba(0.1, 0.2, 0.3, 0.4);
        assert!((c.r - 0.1).abs() < f32::EPSILON);
        assert!((c.g - 0.2).abs() < f32::EPSILON);
        assert!((c.b - 0.3).abs() < f32::EPSILON);
        assert!((c.a - 0.4).abs() < f32::EPSILON);
    }

    // --- AddTextParams builder edge cases ---

    #[test]
    fn test_add_text_params_default_values() {
        let params = AddTextParams::new("text", FontData::Hack(1), "font");
        assert!((params.font_size - 12.0).abs() < f32::EPSILON);
        assert!((params.x - 0.0).abs() < f32::EPSILON);
        assert!((params.y - 0.0).abs() < f32::EPSILON);
        assert_eq!(params.color, PdfColor::BLACK);
        assert!((params.rotation - 0.0).abs() < f32::EPSILON);
        assert_eq!(params.h_align, HAlign::Left);
        assert_eq!(params.v_align, VAlign::Baseline);
        assert!(params.layer_over);
        assert!(!params.strikeout);
        assert!(!params.underline);
    }

    #[test]
    fn test_add_text_params_strikeout_underline() {
        let params = AddTextParams::new("text", FontData::Hack(1), "font")
            .strikeout(true)
            .underline(true);
        assert!(params.strikeout);
        assert!(params.underline);
    }

    #[test]
    fn test_draw_rect_params_builder() {
        let params = DrawRectParams::new(10.0, 20.0, 100.0, 50.0)
            .color(PdfColor::RED)
            .layer_over(false);
        assert!((params.x - 10.0).abs() < f32::EPSILON);
        assert!((params.y - 20.0).abs() < f32::EPSILON);
        assert!((params.width - 100.0).abs() < f32::EPSILON);
        assert!((params.height - 50.0).abs() < f32::EPSILON);
        assert_eq!(params.color, PdfColor::RED);
        assert!(!params.layer_over);
    }

    #[test]
    fn test_draw_rect_params_defaults() {
        let params = DrawRectParams::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(params.color, PdfColor::BLACK);
        assert!(params.layer_over);
    }

    #[test]
    fn test_draw_line_params_builder() {
        let params = DrawLineParams::new(0.0, 0.0, 100.0, 200.0)
            .line_width(2.5)
            .color(PdfColor::RED)
            .layer_over(false);
        assert!((params.x1 - 0.0).abs() < f32::EPSILON);
        assert!((params.y1 - 0.0).abs() < f32::EPSILON);
        assert!((params.x2 - 100.0).abs() < f32::EPSILON);
        assert!((params.y2 - 200.0).abs() < f32::EPSILON);
        assert!((params.line_width - 2.5).abs() < f32::EPSILON);
        assert_eq!(params.color, PdfColor::RED);
        assert!(!params.layer_over);
    }

    #[test]
    fn test_draw_line_params_defaults() {
        let params = DrawLineParams::new(0.0, 0.0, 10.0, 10.0);
        assert!((params.line_width - 1.0).abs() < f32::EPSILON);
        assert_eq!(params.color, PdfColor::BLACK);
        assert!(params.layer_over);
    }
}
