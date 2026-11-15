# medpdf-image

Image embedding companion crate for [medpdf](https://crates.io/crates/medpdf).

Provides high-level functions for embedding raster images (JPEG, PNG, GIF, BMP, TIFF, WebP) and optionally SVG into PDF documents built with lopdf.

## Features

- **JPEG pass-through** -- JPEG files are embedded directly as DCTDecode streams without re-encoding
- **PNG/other formats** -- decoded and embedded as FlateDecode streams with alpha support
- **SVG** -- optional `svg` feature converts SVG to PDF vector content via svg2pdf
- **Fit modes** -- `Stretch`, `Contain`, `Cover` when both width and height are specified
- **DPI limiting** -- automatic downscaling to a configurable max DPI
- **Alpha/opacity** -- per-image alpha via ExtGState
- **Rotation** -- arbitrary rotation around the image anchor point

## Installation

```toml
[dependencies]
medpdf-image = "0.2.2"

# Optional SVG support
medpdf-image = { version = "0.2.2", features = ["svg"] }
```

## Quick Start

```rust
use lopdf::Document;
use medpdf_image::{DrawImageParams, ImageFit, load_image};

let mut doc = Document::load("input.pdf")?;
let page_id = doc.page_iter().next().unwrap();

let image_data = load_image(std::path::Path::new("logo.png"))?;

let params = DrawImageParams {
    x: 72.0,
    y: 700.0,
    width: Some(200.0),
    height: None,
    fit: ImageFit::Contain,
    max_dpi: 300,
    alpha: 1.0,
    rotation: 0.0,
    layer_over: true,
};

medpdf_image::draw_image(&mut doc, page_id, &image_data, &params)?;
doc.save("output.pdf")?;
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
