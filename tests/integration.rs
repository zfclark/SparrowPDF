//! Integration tests for SparrowPDF.
//!
//! These run against the library facade (`src/lib.rs`) so no GUI is started.
//! Every test writes fixtures into a fresh temp directory and cleans up after
//! itself; PDF fixtures are generated on the fly with `lopdf` to avoid
//! committing binary blobs to the repository.

use std::path::{Path, PathBuf};

use sparrowpdf::engine::pdf_io;
use sparrowpdf::model::{InputFile, PageRangeMode, ToolError, ToolId, ToolOutcome};
use sparrowpdf::tools::{tool_for, CancelFlag, Progress, ToolOptions, ToolRequest};

// ---------------------------------------------------------------- fixtures

/// A scratch directory unique per test.
///
/// Prefers cargo's `target/tmp` (CARGO_TARGET_TMPDIR): it sits inside the
/// workspace, which keeps the suite working under restricted environments
/// where the system temp directory is not writable.
fn scratch(tag: &str) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().display().to_string()
    });
    let dir = PathBuf::from(base).join(format!(
        "sparrowpdf-test-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Build a minimal N-page PDF on disk and return its path.
/// `rotation` becomes the page-level /Rotate value.
fn make_pdf(dir: &Path, name: &str, pages: u32, rotation: u32) -> PathBuf {
    use lopdf::{dictionary, Dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => pages as i64,
        "Kids" => Object::Array(Vec::new()),
    });
    let mut kids = Vec::new();
    for i in 0..pages {
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("BT /F1 24 Tf 72 720 Td (Page {i}) Tj ET").into_bytes(),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Rotate" => rotation as i64,
            "Contents" => content_id,
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
            },
        });
        kids.push(page_id.into());
    }
    if let Object::Dictionary(d) = &mut doc.objects.get_mut(&pages_id).unwrap() {
        d.set("Kids", Object::Array(kids));
    }
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.max_id = doc.objects.len() as u32;

    let path = dir.join(name);
    doc.save(&path).expect("save fixture");
    path
}

// ---------------------------------------------------------------- harness

fn request(tool: ToolId, inputs: Vec<PathBuf>, output: PathBuf) -> ToolRequest {
    ToolRequest {
        inputs: inputs.into_iter().map(InputFile::new).collect(),
        output,
        options: ToolOptions::default_for(tool),
        password: None,
    }
}

fn run(tool: ToolId, req: &ToolRequest) -> Result<ToolOutcome, ToolError> {
    let (tx, _rx) = std::sync::mpsc::channel();
    let progress = Progress::new(tx);
    let cancel: CancelFlag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    tool_for(tool).execute(req, &progress, &cancel)
}

// ---------------------------------------------------------------- pdf_io

#[test]
fn parse_pages_explicit_and_ranges() {
    // parse_pages returns 1-based page numbers (see its doc comment).
    assert_eq!(pdf_io::parse_pages("1,3,5", 10).unwrap(), vec![1, 3, 5]);
    assert_eq!(pdf_io::parse_pages("2-4", 10).unwrap(), vec![2, 3, 4]);
    assert_eq!(
        pdf_io::parse_pages("1-2,5,7-8", 10).unwrap(),
        vec![1, 2, 5, 7, 8]
    );
    assert_eq!(pdf_io::parse_pages("all", 4).unwrap(), vec![1, 2, 3, 4]);
}

#[test]
fn parse_pages_rejects_empty_and_out_of_range() {
    // Out-of-range and malformed specs must be rejected; note "7-" is legal
    // in this implementation (shorthand for 7..=total).
    assert!(pdf_io::parse_pages("99", 10).is_err());
    assert!(pdf_io::parse_pages("11", 10).is_err());
    assert!(pdf_io::parse_pages("", 10).is_err());
    assert!(pdf_io::parse_pages("0", 10).is_err());
    assert!(pdf_io::parse_pages("11", 10).is_err());
}

// ---------------------------------------------------------------- tools

#[test]
fn merge_tool_combines_page_counts() {
    let dir = scratch("merge");
    let a = make_pdf(&dir, "a.pdf", 2, 0);
    let b = make_pdf(&dir, "b.pdf", 3, 0);
    let out = dir.join("merged.pdf");

    let req = request(ToolId::Merge, vec![a.clone(), b.clone()], out.clone());
    let outcome = run(ToolId::Merge, &req).expect("merge succeeds");
    assert_eq!(outcome.outputs.len(), 1);

    let count = pdf_io::page_count_of(&out, None).unwrap();
    assert_eq!(count, 5, "merged PDF must have 2+3 pages");
    assert_eq!(pdf_io::page_count_of(&a, None).unwrap(), 2, "source intact");
    assert_eq!(pdf_io::page_count_of(&b, None).unwrap(), 3, "source intact");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rotate_tool_accumulates_and_reopens() {
    let dir = scratch("rotate");
    let src = make_pdf(&dir, "r.pdf", 1, 90);
    let out = dir.join("rotated.pdf");

    let req = request(ToolId::Rotate, vec![src.clone()], out.clone());
    run(ToolId::Rotate, &req).expect("rotate succeeds");

    let doc = pdf_io::open_document(&out, None).unwrap();
    let pages = doc.get_pages();
    let page_id = *pages.values().next().unwrap();
    // 90 (source) + 90 (delta, default option) = 180.
    assert_eq!(pdf_io::page_rotation(&doc, page_id), 180);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_pages_removes_selected_pages() {
    let dir = scratch("delete");
    let src = make_pdf(&dir, "d.pdf", 5, 0);
    let out = dir.join("deleted.pdf");

    let mut req = request(ToolId::DeletePages, vec![src.clone()], out.clone());
    if let ToolOptions::DeletePages { pages } = &mut req.options {
        *pages = "2,4".to_string(); // 1-based: drop pages 2 and 4.
    }
    run(ToolId::DeletePages, &req).expect("delete succeeds");

    let count = pdf_io::page_count_of(&out, None).unwrap();
    assert_eq!(count, 3, "5 pages minus 2 deleted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn encrypt_round_trip_reopens_encrypted() {
    let dir = scratch("encrypt");
    let src = make_pdf(&dir, "e.pdf", 2, 0);
    let out = dir.join("locked.pdf");

    let mut req = request(ToolId::Encrypt, vec![src.clone()], out.clone());
    if let ToolOptions::Encrypt {
        user_password,
        owner_password,
        ..
    } = &mut req.options
    {
        *user_password = "sparrow-test-密码".to_string();
        *owner_password = "sparrow-test-密码".to_string();
    }
    run(ToolId::Encrypt, &req).expect("encrypt succeeds");

    // No password -> PasswordRequired; wrong password -> WrongPassword.
    match pdf_io::open_document(&out, None) {
        Err(ToolError::PasswordRequired { .. }) => {}
        other => panic!("expected PasswordRequired, got {:?}", other.is_ok()),
    }
    match pdf_io::open_document(&out, Some("wrong")) {
        Err(ToolError::WrongPassword { .. }) => {}
        other => panic!("expected WrongPassword, got {:?}", other.is_ok()),
    }
    // Correct password opens and page count survives the round trip.
    let doc = pdf_io::open_document(&out, Some("sparrow-test-密码")).unwrap();
    assert_eq!(pdf_io::page_count(&doc), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn split_every_n_writes_expected_pages() {
    let dir = scratch("split");
    let src = make_pdf(&dir, "s.pdf", 4, 0);
    let out_dir = dir.join("parts");

    let mut req = request(ToolId::Split, vec![src.clone()], out_dir.clone());
    if let ToolOptions::Split { mode, every_n, .. } = &mut req.options {
        *mode = PageRangeMode::EveryN;
        *every_n = 2;
    }
    let outcome = run(ToolId::Split, &req).expect("split succeeds");
    assert_eq!(outcome.outputs.len(), 2, "4 pages / every-2 = 2 outputs");
    for piece in &outcome.outputs {
        assert_eq!(pdf_io::page_count_of(piece, None).unwrap(), 2);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_text_writes_page_text() {
    let dir = scratch("extract");
    let src = make_pdf(&dir, "t.pdf", 1, 0);
    let out = dir.join("text.txt");

    let req = request(ToolId::ExtractText, vec![src.clone()], out.clone());
    let outcome = run(ToolId::ExtractText, &req).expect("extract succeeds");
    assert_eq!(outcome.outputs.len(), 1);
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(
        body.contains("Page 0"),
        "extracted text must contain the fixture page text"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- model

#[test]
fn tool_ids_are_unique_and_complete() {
    let all = ToolId::ALL;
    assert_eq!(all.len(), 13, "registry fixes exactly 13 tools");
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            assert_ne!(all[i], all[j]);
        }
    }
    for id in all {
        assert!(!id.label().is_empty());
    }
}

// ---------------------------------------------------------------- image->pdf

/// A landscape JPEG fixture (2:1), written with the `image` crate.
fn make_landscape_jpeg(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let img = image::RgbImage::from_fn(800, 400, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    img.save_with_format(&path, image::ImageFormat::Jpeg)
        .expect("write jpeg fixture");
    path
}

/// First page's MediaBox as [x0, y0, x1, y1] and content stream text.
fn first_page_geometry(path: &Path) -> ([f64; 4], String) {
    fn obj_f64(o: &lopdf::Object) -> f64 {
        match o {
            lopdf::Object::Real(v) => *v as f64,
            lopdf::Object::Integer(v) => *v as f64,
            other => panic!("not a number: {other:?}"),
        }
    }

    let doc = lopdf::Document::load(path).expect("load output pdf");
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 1, "one image -> one page");
    let page_id = *pages.values().next().unwrap();
    let page = doc.get_object(page_id).unwrap().as_dict().unwrap();
    let media_box: Vec<f64> = page
        .get(b"MediaBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(obj_f64)
        .collect();
    let content_ref = page.get(b"Contents").unwrap().as_reference().unwrap();
    let content = doc
        .get_object(content_ref)
        .unwrap()
        .as_stream()
        .unwrap()
        .content
        .clone();
    let text = String::from_utf8(content).expect("content stream is ascii");
    let mut box4 = [0.0; 4];
    box4.copy_from_slice(&media_box);
    (box4, text)
}

#[test]
fn image_to_pdf_auto_rotates_landscape_image_into_portrait_page() {
    let dir = scratch("img-rot");
    let img = make_landscape_jpeg(&dir, "wide.jpg");
    let out = dir.join("merged.pdf");

    let mut req = request(ToolId::ImageToPdf, vec![img], out.clone());
    if let ToolOptions::ImageToPdf {
        force_orientation,
        auto_rotate,
    } = &mut req.options
    {
        *force_orientation = Some(sparrowpdf::tools::Orientation::Portrait);
        *auto_rotate = true;
    }
    run(ToolId::ImageToPdf, &req).expect("image to pdf succeeds");

    let (media_box, content) = first_page_geometry(&out);
    // A4 portrait page (595.28 x 841.89).
    assert!((media_box[2] - 595.28).abs() < 0.5, "portrait width, got {media_box:?}");
    assert!((media_box[3] - 841.89).abs() < 0.5, "portrait height, got {media_box:?}");
    // Rotation matrix: a≈0, b<0, c>0, d≈0 (90° turn), filling the page height.
    let nums: Vec<f64> = content
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    assert!(nums.len() >= 6, "content stream must start with a cm matrix");
    let [a, b, c, d, _e, _f] = nums[..6] else {
        panic!("matrix parse");
    };
    assert!(a.abs() < 0.01 && d.abs() < 0.01, "rotated: {nums:?}");
    assert!(b < -100.0 && c > 100.0, "rotated: {nums:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn image_to_pdf_without_auto_rotate_keeps_image_upright() {
    let dir = scratch("img-norot");
    let img = make_landscape_jpeg(&dir, "wide.jpg");
    let out = dir.join("merged.pdf");

    let mut req = request(ToolId::ImageToPdf, vec![img], out.clone());
    if let ToolOptions::ImageToPdf {
        force_orientation,
        auto_rotate,
    } = &mut req.options
    {
        *force_orientation = Some(sparrowpdf::tools::Orientation::Portrait);
        *auto_rotate = false;
    }
    run(ToolId::ImageToPdf, &req).expect("image to pdf succeeds");

    let (_media_box, content) = first_page_geometry(&out);
    // Identity orientation: a>0, d>0, b≈0, c≈0.
    let nums: Vec<f64> = content
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    assert!(nums.len() >= 6, "content stream must start with a cm matrix");
    let [a, b, c, d, _e, _f] = nums[..6] else {
        panic!("matrix parse");
    };
    assert!(a > 100.0 && d > 100.0, "upright: {nums:?}");
    assert!(b.abs() < 0.01 && c.abs() < 0.01, "upright: {nums:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------ folder batch

/// A small JPEG fixture with the given size, written as `dir/name`.
fn make_image(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
    let path = dir.join(name);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    img.save_with_format(&path, image::ImageFormat::Jpeg)
        .expect("write jpeg fixture");
    path
}

#[test]
fn folder_batch_collects_images_in_numeric_order() {
    let dir = scratch("folder-sort");
    for name in ["10.png", "2.png", "1.png"] {
        make_image(&dir, name, 40, 20);
    }
    std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();

    let images = sparrowpdf::tools::folder_to_pdf::collect_images(&dir);
    let names: Vec<String> = images
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["1.png", "2.png", "10.png"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_batch_writes_one_pdf_per_folder_into_itself() {
    let dir = scratch("folder-batch");
    let album1 = dir.join("album1");
    std::fs::create_dir_all(&album1).unwrap();
    for name in ["1.jpg", "2.jpg", "3.jpg"] {
        make_image(&album1, name, 60, 40);
    }
    let album2 = dir.join("album2");
    std::fs::create_dir_all(&album2).unwrap();
    make_image(&album2, "a.png", 60, 40);
    make_image(&album2, "b.png", 60, 40);
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();

    // Empty output path = default: each PDF lands inside its own folder.
    let req = request(
        ToolId::FolderToPdf,
        vec![album1.clone(), album2.clone(), empty.clone()],
        PathBuf::new(),
    );
    match run(ToolId::FolderToPdf, &req) {
        Err(ToolError::PartialSuccess {
            succeeded, failed, ..
        }) => {
            assert_eq!(succeeded, 2, "two folders have images");
            assert_eq!(failed, 1, "the empty folder is skipped");
        }
        other => panic!("expected PartialSuccess, got {:?}", other.is_ok()),
    }

    let pdf1 = album1.join("album1.pdf");
    let pdf2 = album2.join("album2.pdf");
    assert!(pdf1.is_file() && pdf2.is_file(), "PDFs named after folders");
    assert_eq!(pdf_io::page_count_of(&pdf1, None).unwrap(), 3);
    assert_eq!(pdf_io::page_count_of(&pdf2, None).unwrap(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_batch_two_per_page_merges_specified_pairs() {
    let dir = scratch("folder-pair");
    let album = dir.join("pair");
    std::fs::create_dir_all(&album).unwrap();
    for name in ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"] {
        make_image(&album, name, 80, 40);
    }
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut req = request(ToolId::FolderToPdf, vec![album], out_dir.clone());
    if let ToolOptions::FolderToPdf {
        mode,
        use_merge_spec,
        merge_spec,
        ..
    } = &mut req.options
    {
        *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
        // Images 2+3 share page 1; 1, 4, 5 stay single -> 4 pages.
        *use_merge_spec = true;
        *merge_spec = "2-3".to_string();
    }
    let outcome = run(ToolId::FolderToPdf, &req).expect("batch with spec succeeds");
    assert_eq!(outcome.outputs.len(), 1);
    let pdf = &outcome.outputs[0];
    assert_eq!(pdf_io::page_count_of(pdf, None).unwrap(), 4);

    // Two-per-page pages are landscape A4.
    let doc = lopdf::Document::load(pdf).unwrap();
    let page_id = *doc.get_pages().values().next().unwrap();
    let page = doc.get_object(page_id).unwrap().as_dict().unwrap();
    let media_box: Vec<f64> = page
        .get(b"MediaBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| match o {
            lopdf::Object::Real(v) => *v as f64,
            lopdf::Object::Integer(v) => *v as f64,
            other => panic!("not a number: {other:?}"),
        })
        .collect();
    assert!((media_box[2] - 841.89).abs() < 0.5, "landscape width");
    assert!((media_box[3] - 595.28).abs() < 0.5, "landscape height");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_batch_rejects_bad_merge_specs() {
    let dir = scratch("folder-spec");
    let album = dir.join("spec");
    std::fs::create_dir_all(&album).unwrap();
    for name in ["1.jpg", "2.jpg", "3.jpg"] {
        make_image(&album, name, 40, 20);
    }

    // Empty input counts as a mistake only when the checkbox is checked.
    for bad_spec in ["9", "1-3", "0", "", "  "] {
        let mut req = request(ToolId::FolderToPdf, vec![album.clone()], dir.join("o.pdf"));
        if let ToolOptions::FolderToPdf {
            mode,
            use_merge_spec,
            merge_spec,
            ..
        } = &mut req.options
        {
            *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
            *use_merge_spec = true;
            *merge_spec = bad_spec.to_string();
        }
        match run(ToolId::FolderToPdf, &req) {
            Err(ToolError::InvalidPageRange { .. }) => {}
            other => panic!(
                "spec {bad_spec:?}: expected InvalidPageRange, got {:?}",
                other.is_ok()
            ),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_batch_without_merge_spec_pairs_everything_sequentially() {
    let dir = scratch("folder-nospec");
    let album = dir.join("nospec");
    std::fs::create_dir_all(&album).unwrap();
    for name in ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"] {
        make_image(&album, name, 80, 40);
    }
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    // Unchecked: the layout applies to everything — sequential pairs
    // (1,2) (3,4) (5) — even if a leftover spec string is present.
    let mut req = request(ToolId::FolderToPdf, vec![album], out_dir.clone());
    if let ToolOptions::FolderToPdf {
        mode,
        use_merge_spec,
        merge_spec,
        ..
    } = &mut req.options
    {
        *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
        *use_merge_spec = false;
        *merge_spec = "9-9".to_string(); // must be ignored
    }
    let outcome = run(ToolId::FolderToPdf, &req).expect("batch without spec succeeds");
    assert_eq!(outcome.outputs.len(), 1);
    assert_eq!(pdf_io::page_count_of(&outcome.outputs[0], None).unwrap(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}
