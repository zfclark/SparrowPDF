//! Medium-level PDF API over lopdf.
//!
//! Provides reusable operations for building, merging, and manipulating PDF documents:
//! page copying, deletion, blank page creation, overlays, text/shape watermarking,
//! font discovery and embedding, and document encryption.

pub mod error;
pub mod font_data;
pub mod font_helpers;
pub mod parsing;
pub mod pdf_blank_page;
pub mod pdf_copy_page;
pub mod pdf_delete_page;
pub mod pdf_font;
pub mod pdf_helpers;
pub mod pdf_encryption;
pub mod pdf_overlay;
pub mod pdf_subset;
pub mod pdf_watermark;
pub mod types;

// Re-exports for convenience
pub use error::{Error, MedpdfError, Result};
pub use font_data::FontData;
pub use font_helpers::measure_text_width;
pub use parsing::parse_page_spec;
pub use pdf_blank_page::create_blank_page;
pub use pdf_copy_page::{copy_page, copy_page_with_cache};
pub use pdf_delete_page::delete_page;
pub use pdf_encryption::{encrypt_document, parse_permission_name, parse_permissions, EncryptionAlgorithm, EncryptionParams};
pub use pdf_font::{find_font, find_font_with_style, FontCache, FontPath};
pub use pdf_helpers::{deep_copy_object, deep_copy_object_by_id, get_page_media_box, get_page_rotation, register_in_page_resources, set_page_rotation, Unit, KEY_CONTENTS, KEY_EXTGSTATE, KEY_RESOURCES, KEY_XOBJECT};
pub use pdf_overlay::overlay_page;
pub use pdf_subset::subset_fonts;
pub use pdf_watermark::{add_line, add_rect, add_text_params, insert_content_stream, register_extgstate_in_page_resources, EmbeddedFontCache};
pub use types::{AddTextParams, DrawLineParams, DrawRectParams, FontStyle, FontWeight, HAlign, PdfColor, VAlign};
