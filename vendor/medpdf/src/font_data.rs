use std::sync::Arc;

/// Represents the source/type of font data for PDF text rendering.
///
/// Replaces the previous convention of using sentinel bytes in a `Vec<u8>`
/// to distinguish font types.
#[derive(Debug, Clone)]
pub enum FontData {
    /// Numeric reference to an existing font object in the document (uses key "Fn").
    Hack(u8),
    /// Standard PDF built-in font (e.g. "Helvetica") -- no embedding needed.
    BuiltIn(String),
    /// Embedded TTF/OTF font data.
    Embedded(Arc<Vec<u8>>),
}

impl FontData {
    /// Returns embedded font bytes for TTF parsing, or None for Hack/BuiltIn.
    pub fn embedded_bytes(&self) -> Option<&[u8]> {
        match self {
            FontData::Embedded(data) => Some(data),
            _ => None,
        }
    }
}
