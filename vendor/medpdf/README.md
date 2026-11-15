# medpdf

A medium-level PDF manipulation library built on [lopdf](https://github.com/J-F-Liu/lopdf).

medpdf provides higher-level, reusable operations for common PDF tasks while exposing lopdf's `Document` type for direct manipulation when needed.

## Features

- **Page copying** between documents with full resource handling
- **Page overlay** with automatic resource conflict resolution
- **Text watermarks** with embedded or built-in PDF fonts
- **Blank page creation** with custom dimensions
- **Page specification parsing** (`"1-3,5,7-"`, `"all"`)
- **Font discovery** (system fonts, file paths, built-in fonts)
- **Deep object copying** with reference tracking

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
medpdf = "0.9.2"
```

## Quick Start

```rust
use lopdf::Document;
use medpdf::{copy_page, create_blank_page, parse_page_spec, Result};

fn main() -> Result<()> {
    // Load source document
    let source_doc = Document::load("input.pdf")?;

    // Create destination document
    let mut dest_doc = Document::with_version("1.5");

    // Parse page specification
    let page_count = source_doc.get_pages().len() as u32;
    let pages = parse_page_spec("1-3,5", page_count)?;

    // Copy selected pages
    for page_num in pages {
        copy_page(&mut dest_doc, &source_doc, page_num)?;
    }

    // Add a blank page (letter size: 612x792 points)
    create_blank_page(&mut dest_doc, 612.0, 792.0)?;

    dest_doc.save("output.pdf")?;
    Ok(())
}
```

## API Reference

### Error Handling

```rust
use medpdf::{Error, MedpdfError, Result};
```

`MedpdfError` wraps errors from underlying libraries:

| Variant | Source |
|---------|--------|
| `Io` | `std::io::Error` |
| `LoPdf` | `lopdf::Error` |
| `FontKit` | `font_kit::error::SelectionError` |
| `Face` | `ttf_parser::FaceParsingError` |
| `Message` | Custom error messages |

All public functions return `Result<T>`, which is `std::result::Result<T, MedpdfError>`.

### Page Operations

#### `copy_page`

Copies a page from a source document to a destination document, including all referenced objects (fonts, images, etc.).

```rust
use medpdf::copy_page;

let new_page_id = copy_page(&mut dest_doc, &source_doc, 1)?;
```

Note: Each call creates its own reference tracking map. Use `copy_page_with_cache` when copying multiple pages to deduplicate shared resources.

#### `copy_page_with_cache`

Copies a page using a shared cache to avoid duplicating resources (fonts, images, etc.) across multiple pages.

```rust
use medpdf::copy_page_with_cache;
use std::collections::BTreeMap;

let mut cache = BTreeMap::new();
for page_num in 1..=10 {
    copy_page_with_cache(&mut dest_doc, &source_doc, page_num, &mut cache)?;
}
```

The cache maps source object IDs to destination object IDs. Pass the same cache to all `copy_page_with_cache` calls when copying from the same source document.

#### `create_blank_page`

Creates a blank page with specified dimensions (in points).

```rust
use medpdf::create_blank_page;

// Letter size (8.5" x 11" at 72 dpi)
let page_id = create_blank_page(&mut dest_doc, 612.0, 792.0)?;

// A4 size
let page_id = create_blank_page(&mut dest_doc, 595.0, 842.0)?;
```

#### `overlay_page`

Overlays content from one page onto another, automatically renaming resources to prevent conflicts.

```rust
use medpdf::overlay_page;

overlay_page(
    &mut dest_doc,
    dest_page_id,      // Target page ObjectId
    &overlay_doc,      // Source document
    1,                 // Source page number (1-based)
)?;
```

Resource key deduplication recursively scans the entire destination page tree (both `/Pages` and `/Page` nodes) to collect existing resource names before generating unique overlay names.

#### `add_text_params`

Adds a text watermark to a page with full control over color, alignment, rotation, and more.

```rust
use medpdf::{add_text_params, AddTextParams, PdfColor, HAlign, EmbeddedFontCache};

let mut font_cache = EmbeddedFontCache::new();
let params = AddTextParams::new("DRAFT", font_data, "Helvetica")
    .font_size(24.0)
    .position(100.0, 100.0)
    .color(PdfColor::rgba(1.0, 0.0, 0.0, 0.5))  // Semi-transparent red
    .rotation(45.0)
    .h_align(HAlign::Center)
    .layer_over(true);

add_text_params(&mut dest_doc, page_id, &params, &mut font_cache)?;
```

### Parsing

#### `parse_page_spec`

Parses page range specifications into a sorted vector of page numbers.

```rust
use medpdf::parse_page_spec;

let pages = parse_page_spec("1-3,5,7-", 10)?;
// Returns: [1, 2, 3, 5, 7, 8, 9, 10]

let all_pages = parse_page_spec("all", 10)?;
// Returns: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

Supported syntax:
- Single pages: `"5"`
- Ranges: `"1-5"`
- Open start: `"-5"` (pages 1 through 5)
- Open end: `"5-"` (page 5 through last)
- Lists: `"1,3,5-7,10"`
- All pages: `"all"`

### Font Handling

```rust
use medpdf::pdf_font::{FontPath, FontCache, find_font};
use std::path::Path;
```

#### `FontPath`

Represents the source of a font:

| Variant | Description |
|---------|-------------|
| `BuiltIn(String)` | PDF built-in font (Helvetica, Courier, etc.) |
| `Path(PathBuf)` | Path to a font file |
| `Memory(Arc<Vec<u8>>, String)` | In-memory font data with display name |
| `Hack(u8)` | Reference to existing document font by index |

#### `find_font`

Discovers fonts by name or path:

```rust
// Built-in fonts (prefixed with @)
let font = find_font(Path::new("@Helvetica"))?;

// System font search
let font = find_font(Path::new("Arial"))?;

// Direct file path
let font = find_font(Path::new("/path/to/font.ttf"))?;
```

Built-in PDF fonts (PDF 1.7):
- `@Helvetica`, `@Helvetica-Bold`, `@Helvetica-Oblique`, `@Helvetica-BoldOblique`
- `@Courier`, `@Courier-Bold`, `@Courier-Oblique`, `@Courier-BoldOblique`
- `@Times-Roman`, `@Times-Bold`, `@Times-Italic`, `@Times-BoldItalic`
- `@Symbol`, `@ZapfDingbats`

#### `FontCache`

Caches loaded font data to avoid repeated file reads:

```rust
let mut cache = FontCache::new();
let font_data = cache.get_data(&font_path)?;
```

### Unit Conversion

```rust
use medpdf::Unit;

let points = Unit::In.to_points(1.0);  // 72.0
let points = Unit::Mm.to_points(25.4); // 72.0
```

### PDF Key Constants

`medpdf::pdf_helpers` exports byte-string constants for common PDF dictionary keys:

```rust
use medpdf::pdf_helpers::{KEY_PAGES, KEY_RESOURCES, KEY_CONTENTS, KEY_FONT};
```

### Deep Copy (Internal)

`deep_copy_object()` and `deep_copy_object_by_id()` are `pub(crate)` helpers used internally by `copy_page`, `overlay_page`, and other operations. They recursively clone PDF objects using a `BTreeMap<ObjectId, ObjectId>` to track copies, preventing duplicates and maintaining reference integrity. `Parent` references are skipped to avoid copying the entire page tree.

## Key Patterns

### Resource Renaming

When overlaying pages, resources (fonts, images, etc.) may have conflicting names. medpdf automatically renames overlay resources with a `_o` suffix and updates content stream references.

### Graphics State Isolation

Content streams are wrapped with `q` (save) and `Q` (restore) operators to isolate graphics state changes. Unbalanced `q`/`Q` pairs in source documents are detected and corrected.

### Reference Tracking

Deep copy operations use a `BTreeMap<ObjectId, ObjectId>` to track copied objects. This ensures:
- Each source object is copied exactly once
- References are updated to point to new object IDs
- `Parent` references are skipped to avoid copying the entire document tree

## Relationship to lopdf

medpdf is built on lopdf and re-exports nothing from it. You'll typically use both together:

```rust
use lopdf::Document;
use medpdf::{copy_page, Result};
```

**Use lopdf directly for:**
- Loading and saving documents
- Low-level object manipulation
- Creating new PDF structures

**Use medpdf for:**
- Copying pages between documents
- Overlaying content with resource management
- Adding watermarks
- Parsing page specifications

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
