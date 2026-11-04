//! Integration tests for SparrowPDF.
//!
//! These run against the library facade (`src/lib.rs`) so no GUI is started.
//! Every test writes fixtures into a fresh temp directory and cleans up after
//! itself; PDF fixtures are generated on the fly with `lopdf` to avoid
//! committing binary blobs to the repository.

use std::path::{Path, PathBuf};

use sparrowpdf::engine::pdf_io;
use sparrowpdf::model::{ConflictPolicy, InputFile, PageRangeMode, ToolError, ToolId, ToolOutcome};
use sparrowpdf::tools::{tool_for, CancelFlag, Progress, ToolOptions, ToolRequest};

// ---------------------------------------------------------------- fixtures

/// Directory that holds every scratch folder.
///
/// `CARGO_TARGET_TMPDIR` is set in the *compile* environment only — cargo does
/// not export it to the test process — so it must be read with `env!`. Reading
/// it at run time always fell through to the system temp directory, which is
/// unwritable in restricted environments (and contradicts the rule that test
/// data stays inside the workspace).
fn scratch_base() -> PathBuf {
    if let Some(base) = option_env!("CARGO_TARGET_TMPDIR") {
        return PathBuf::from(base);
    }
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    let _ = std::fs::create_dir_all(&base);
    base
}

/// A scratch directory unique per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = scratch_base().join(format!("sparrowpdf-test-{}-{}", tag, std::process::id()));
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
        // The default the UI ships with: report a clash, never overwrite.
        conflict: ConflictPolicy::Report,
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

// ---------------------------------------------------------------- logging

/// CJK watermarks are stamped as transparent bitmaps (simple PDF fonts are
/// WinAnsi-encoded and would turn every CJK char into `?`), so the output
/// must contain an image XObject whose SMask actually carries glyph shapes.
#[test]
fn text_watermark_cjk_writes_bitmap_watermark() {
    let dir = scratch("wm-cjk");
    let src = make_pdf(&dir, "in.pdf", 1, 0);
    let out = dir.join("wm.pdf");
    let mut req = request(ToolId::TextWatermark, vec![src], out.clone());
    if let ToolOptions::TextWatermark { text, opacity, .. } = &mut req.options {
        *text = "机密文件".to_string();
        *opacity = 0.9;
    }
    let outcome = run(ToolId::TextWatermark, &req).expect("watermark succeeds");
    assert_eq!(outcome.outputs.len(), 1);

    let doc = pdf_io::open_document(&out, None).unwrap();
    let mut found_image = false;
    let mut glyph_coverage = 0u64;
    for object_id in doc.objects.values() {
        let Ok(stream) = object_id.as_stream() else {
            continue;
        };
        let dict = &stream.dict;
        let is_image =
            matches!(dict.get(b"Subtype"), Ok(lopdf::Object::Name(n)) if n.as_slice() == b"Image");
        if !is_image {
            continue;
        }
        // The SMask itself is a DeviceGray image XObject; the watermark tint
        // is the DeviceRGB one carrying the SMask reference.
        let is_gray = matches!(
            dict.get(b"ColorSpace"),
            Ok(lopdf::Object::Name(n)) if n.as_slice() == b"DeviceGray"
        );
        if is_gray {
            continue;
        }
        found_image = true;
        assert!(
            matches!(dict.get(b"SMask"), Ok(lopdf::Object::Reference(_))),
            "watermark image must carry an SMask alpha channel"
        );
        let smask_ref = dict.get(b"SMask").unwrap().as_reference().unwrap();
        let smask = doc.get_object(smask_ref).unwrap().as_stream().unwrap();
        let coverage = smask.decompressed_content().expect("smask flate decodes");
        glyph_coverage += coverage.iter().map(|&b| (b > 32) as u64).sum::<u64>();
    }
    assert!(found_image, "output must embed a watermark image");
    assert!(
        glyph_coverage > 1000,
        "SMask must contain real glyph shapes, got {glyph_coverage} covered pixels"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn log_lines_are_appended_to_the_log_file() {
    // Redirect the log into the workspace so the test does not depend on
    // AppData being writable from this process.
    let dir = scratch_base();
    std::env::set_var("SPARROW_LOG_DIR", &dir);

    let path = sparrowpdf::engine::logging::log_path();
    let before = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .count();
    sparrowpdf::engine::logging::log("TEST", "integration-log-probe");
    let after = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = after.lines().collect();
    assert!(
        lines.len() > before,
        "log file must gain a line: {}",
        path.display()
    );
    assert!(
        lines.last().unwrap().contains("integration-log-probe"),
        "last log line must contain the probe message"
    );
    assert!(lines.last().unwrap().contains("[TEST]"));
}

// ---------------------------------------------------------------- model

#[test]
fn tool_ids_are_unique_and_complete() {
    let all = ToolId::ALL;
    assert_eq!(all.len(), 12, "registry fixes exactly 12 tools");
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
    assert!(
        (media_box[2] - 595.28).abs() < 0.5,
        "portrait width, got {media_box:?}"
    );
    assert!(
        (media_box[3] - 841.89).abs() < 0.5,
        "portrait height, got {media_box:?}"
    );
    // Rotation matrix: a≈0, b<0, c>0, d≈0 (90° turn), filling the page height.
    let nums: Vec<f64> = content
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    assert!(
        nums.len() >= 6,
        "content stream must start with a cm matrix"
    );
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
    assert!(
        nums.len() >= 6,
        "content stream must start with a cm matrix"
    );
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
            succeeded,
            failed,
            outputs,
            ..
        }) => {
            assert_eq!(succeeded, 2, "two folders have images");
            assert_eq!(failed, 1, "the empty folder is skipped");
            // A partial run produced files; the result card must be able to
            // offer them instead of reporting a bare failure.
            assert_eq!(outputs.len(), 2, "partial success carries its outputs");
            for output in &outputs {
                assert!(output.is_file(), "{} must exist", output.display());
            }
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
fn preview_page_numbers_renumber_by_output_page() {
    use sparrowpdf::tools::folder_to_pdf::preview_page_numbers;
    // Spec "2-3" with 5 images: pages [1], [2+3], [4], [5] — inline order,
    // continuous page numbers, merged images share one label.
    assert_eq!(
        preview_page_numbers(5, true, true, "2-3"),
        vec![1, 2, 2, 3, 4]
    );
    // Unchecked: sequential pairs over everything.
    assert_eq!(
        preview_page_numbers(5, true, false, "9-9"),
        vec![1, 1, 2, 2, 3]
    );
    // Single mode: every image its own page, spec ignored.
    assert_eq!(preview_page_numbers(3, false, true, "1-2"), vec![1, 2, 3]);
    // Invalid spec: the preview falls back to sequential pairing.
    assert_eq!(preview_page_numbers(4, true, true, "9"), vec![1, 1, 2, 2]);
}

#[test]
fn folder_batch_two_per_page_supports_portrait_pages() {
    let dir = scratch("folder-portrait");
    let album = dir.join("portrait-pair");
    std::fs::create_dir_all(&album).unwrap();
    make_image(&album, "1.jpg", 60, 90);
    make_image(&album, "2.jpg", 60, 90);
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut req = request(ToolId::FolderToPdf, vec![album], out_dir.clone());
    if let ToolOptions::FolderToPdf {
        mode,
        pair_orientation,
        ..
    } = &mut req.options
    {
        *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
        *pair_orientation = sparrowpdf::tools::Orientation::Portrait;
    }
    let outcome = run(ToolId::FolderToPdf, &req).expect("portrait pair succeeds");
    assert_eq!(outcome.outputs.len(), 1);

    // Portrait A4 page holding both images (two /Do draw ops).
    let doc = lopdf::Document::load(&outcome.outputs[0]).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
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
    assert!((media_box[2] - 595.28).abs() < 0.5, "portrait width");
    assert!((media_box[3] - 841.89).abs() < 0.5, "portrait height");
    let content_ref = page.get(b"Contents").unwrap().as_reference().unwrap();
    let content = String::from_utf8(
        doc.get_object(content_ref)
            .unwrap()
            .as_stream()
            .unwrap()
            .content
            .clone(),
    )
    .unwrap();
    assert_eq!(content.matches(" Do").count(), 2, "both slots drawn");
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

/// MediaBox of every page, in page order, as `[x0, y0, x1, y1]`.
fn page_media_boxes(path: &Path) -> Vec<[f64; 4]> {
    fn num(o: &lopdf::Object) -> f64 {
        match o {
            lopdf::Object::Real(v) => *v as f64,
            lopdf::Object::Integer(v) => *v as f64,
            other => panic!("not a number: {other:?}"),
        }
    }

    let doc = lopdf::Document::load(path).expect("load output pdf");
    let mut boxes: Vec<(u32, [f64; 4])> = Vec::new();
    for (page_num, page_id) in doc.get_pages() {
        let page = doc.get_object(page_id).unwrap().as_dict().unwrap();
        let values: Vec<f64> = page
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(num)
            .collect();
        boxes.push((page_num, [values[0], values[1], values[2], values[3]]));
    }
    boxes.sort_by_key(|(page_num, _)| *page_num);
    boxes.into_iter().map(|(_, b)| b).collect()
}

/// Raw content stream of the n-th page (1-based).
fn page_content(path: &Path, page_num: u32) -> String {
    let doc = lopdf::Document::load(path).expect("load output pdf");
    let page_id = *doc.get_pages().get(&page_num).expect("page exists");
    let page = doc.get_object(page_id).unwrap().as_dict().unwrap();
    let content_ref = page.get(b"Contents").unwrap().as_reference().unwrap();
    String::from_utf8(
        doc.get_object(content_ref)
            .unwrap()
            .as_stream()
            .unwrap()
            .content
            .clone(),
    )
    .expect("content stream is ascii")
}

/// First `cm` matrix of a page's content stream as `[a, b, c, d, e, f]`.
fn first_matrix(path: &Path, page_num: u32) -> [f64; 6] {
    let content = page_content(path, page_num);
    let nums: Vec<f64> = content
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    assert!(
        nums.len() >= 6,
        "page {page_num} must start with a cm matrix"
    );
    [nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]]
}

/// Regression: in two-per-page mode the A4 orientation is a document-wide
/// setting, so the pages that end up holding a single image (images the merge
/// spec leaves alone) must use it too. Before the fix only merged pages
/// followed `pair_orientation` while single pages fell back to each image's
/// own orientation, producing a mixed-orientation PDF.
#[test]
fn folder_batch_pair_portrait_applies_to_single_image_pages() {
    let dir = scratch("folder-pair-all-portrait");
    let album = dir.join("mixed");
    std::fs::create_dir_all(&album).unwrap();
    // Landscape images: the old per-image fallback would make the single
    // pages landscape.
    for name in ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"] {
        make_image(&album, name, 90, 60);
    }
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut req = request(ToolId::FolderToPdf, vec![album], out_dir.clone());
    if let ToolOptions::FolderToPdf {
        mode,
        pair_orientation,
        use_merge_spec,
        merge_spec,
        ..
    } = &mut req.options
    {
        *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
        *pair_orientation = sparrowpdf::tools::Orientation::Portrait;
        *use_merge_spec = true;
        *merge_spec = "2-3".to_string();
    }
    let outcome = run(ToolId::FolderToPdf, &req).expect("portrait batch succeeds");

    // Pages: [1], [2+3], [4], [5].
    let boxes = page_media_boxes(&outcome.outputs[0]);
    assert_eq!(boxes.len(), 4);
    for (index, b) in boxes.iter().enumerate() {
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        assert!(
            (w - 595.28).abs() < 0.5 && (h - 841.89).abs() < 0.5,
            "page {} must be A4 portrait, got {w}x{h}",
            index + 1
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same rule for landscape pages, plus the outer margin: a single-image
/// page must be inset like the merged pages instead of bleeding to the edge.
#[test]
fn folder_batch_pair_landscape_keeps_single_pages_inset() {
    let dir = scratch("folder-pair-all-landscape");
    let album = dir.join("mixed");
    std::fs::create_dir_all(&album).unwrap();
    for name in ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"] {
        make_image(&album, name, 60, 90);
    }
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut req = request(ToolId::FolderToPdf, vec![album], out_dir.clone());
    if let ToolOptions::FolderToPdf {
        mode,
        pair_orientation,
        use_merge_spec,
        merge_spec,
        auto_rotate,
        ..
    } = &mut req.options
    {
        *mode = sparrowpdf::tools::BatchPageMode::TwoPerPage;
        *pair_orientation = sparrowpdf::tools::Orientation::Landscape;
        *use_merge_spec = true;
        *merge_spec = "2-3".to_string();
        // Keep the image upright so the placement matrix stays axis-aligned.
        *auto_rotate = false;
    }
    let outcome = run(ToolId::FolderToPdf, &req).expect("landscape batch succeeds");

    let boxes = page_media_boxes(&outcome.outputs[0]);
    assert_eq!(boxes.len(), 4);
    for (index, b) in boxes.iter().enumerate() {
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        assert!(
            (w - 841.89).abs() < 0.5 && (h - 595.28).abs() < 0.5,
            "page {} must be A4 landscape, got {w}x{h}",
            index + 1
        );
    }

    // Page 1 holds image 1 alone; its draw origin must sit inside the margin.
    let [a, b, _c, d, _e, f] = first_matrix(&outcome.outputs[0], 1);
    assert!(a > 0.0 && d > 0.0, "image stays upright: a={a}, d={d}");
    assert!(b.abs() < 0.01, "no rotation applied: b={b}");
    assert!(
        f > 10.0,
        "single-image page must keep the outer margin, got y0={f}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reorder preview labels pages by their position in the composed order,
/// using the same mapping the tool copies pages with.
#[test]
fn reorder_ordered_pages_maps_source_order_to_output_pages() {
    use sparrowpdf::tools::reorder::ordered_pages;

    // Source pages 3, 1, 2 -> output pages 1, 2, 3.
    assert_eq!(ordered_pages(&[2, 0, 1], 3), vec![3, 1, 2]);
    // Out-of-range indices are dropped (the tool rejects the run instead).
    assert_eq!(ordered_pages(&[0, 5, 1], 3), vec![1, 2]);
    assert!(ordered_pages(&[], 3).is_empty());
}

/// A PDF whose pages differ in both fill tone and bar position, so rendered
/// page images can be compared: identical pages would make an order check
/// vacuous.
fn make_marked_pdf(dir: &Path, name: &str, pages: u32) -> PathBuf {
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => pages as i64,
        "Kids" => Object::Array(Vec::new()),
    });
    let mut kids = Vec::new();
    for i in 0..pages {
        let tone = 0.1 + 0.1 * i as f32;
        let bar_y = 60.0 + 120.0 * i as f32;
        let content = format!(
            "q {tone} {tone} {tone} rg 0 0 612 792 re f Q\nq 0 0 0 rg 0 {bar_y} 612 40 re f Q\n"
        );
        let content_id = doc.add_object(Stream::new(Default::default(), content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! {},
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

/// The reorder preview must show exactly the pages the saved PDF contains, in
/// the composed order. Both sides go through the same rasterizer, so pixel
/// equality proves the preview's page sequence is the output page sequence.
#[test]
fn reorder_preview_order_matches_saved_pdf_pages() {
    use sparrowpdf::engine::render;
    use sparrowpdf::tools::reorder::ordered_pages;

    let dir = scratch("reorder-preview");
    let src = make_marked_pdf(&dir, "pages.pdf", 4);
    let out = dir.join("reordered.pdf");

    // Drag order: source pages 3, 1, 4, 2 (0-based [2, 0, 3, 1]).
    let order = vec![2usize, 0, 3, 1];
    let mut req = request(ToolId::Reorder, vec![src.clone()], out.clone());
    if let ToolOptions::Reorder { order: opt } = &mut req.options {
        *opt = order.clone();
    }
    run(ToolId::Reorder, &req).expect("reorder succeeds");

    let source_order = ordered_pages(&order, 4);
    assert_eq!(source_order, vec![3, 1, 4, 2], "composed output order");

    let fonts = render::FontSet::system();
    let preview = render::render_page_thumbnails(&src, None, &source_order, 100.0, &fonts)
        .expect("render source pages");
    let saved = render::render_page_thumbnails(&out, None, &[1, 2, 3, 4], 100.0, &fonts)
        .expect("render output pages");

    assert_eq!(preview.len(), 4);
    assert_eq!(saved.len(), 4);
    // Fixture sanity: pages must actually differ, otherwise the check is moot.
    assert_ne!(
        preview[0].1.data, preview[1].1.data,
        "fixture pages must render differently"
    );
    for (index, ((_, preview_page), (_, saved_page))) in
        preview.iter().zip(saved.iter()).enumerate()
    {
        assert_eq!(
            (preview_page.width, preview_page.height),
            (saved_page.width, saved_page.height),
            "page {} size differs between preview and output",
            index + 1
        );
        assert!(
            preview_page.data == saved_page.data,
            "page {} of the preview does not match the saved output",
            index + 1
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression: fills used to drop the wrap-around edge of a closed subpath, so
/// an axis-aligned `re` had a single crossing edge and never painted; and the
/// vertical sub-samples were composited one by one, washing solid areas out to
/// ~68%. Both surfaced as blank or pale preview thumbnails.
#[test]
fn renderer_fills_closed_axis_aligned_paths_at_full_strength() {
    use sparrowpdf::engine::render;

    let dir = scratch("raster-fill");
    let pdf = make_marked_pdf(&dir, "filled.pdf", 2);
    let fonts = render::FontSet::system();
    let pages = render::render_page_thumbnails(&pdf, None, &[1], 120.0, &fonts)
        .expect("render filled page");

    let pixmap = &pages[0].1;
    let middle = ((pixmap.height / 2) * pixmap.width + pixmap.width / 2) as usize * 4;
    let tone = pixmap.data[middle];
    // Page 1 is filled with 10% grey (~26 of 255): white means no fill at all,
    // anything much lighter means the coverage was composed per sub-sample.
    assert!(
        (tone as i32 - 26).abs() <= 6,
        "expected a solid 10% grey fill, got {tone}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A one-page PDF whose only content is a **raw** (unfiltered) RGB image
/// XObject, drawn through a `cm` that scales it up.
fn make_raw_image_pdf(dir: &Path, name: &str, page: f32) -> PathBuf {
    use lopdf::{dictionary, Document, Stream};

    let mut doc = Document::with_version("1.5");
    let (w, h) = (8u32, 8u32);
    let mut pixels: Vec<u8> = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..(w * h) {
        pixels.extend_from_slice(&[0, 0, 255]); // solid blue, no /Filter
    }
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8i64,
        },
        pixels,
    ));
    // Draw the unit image square across most of the page.
    let content_id = doc.add_object(Stream::new(
        Default::default(),
        format!("q {} 0 0 {} 20 20 cm /Im1 Do Q", page - 40.0, page - 40.0).into_bytes(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), page.into(), page.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image_id } },
    });
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => 1i64,
        "Kids" => vec![page_id.into()],
    });
    if let Ok(dict) = doc.get_object_mut(page_id).unwrap().as_dict_mut() {
        dict.set("Parent", pages_id);
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

/// Regression, two bugs at once:
///
/// * the matrix product was computed in the wrong convention, so any `cm`
///   before `Do`/`Tj` landed the geometry a page-height away (blank page);
/// * raw image streams (no `/Filter`) were dropped because
///   `decompressed_content` rejects them, so every PNG-sourced page rendered
///   blank in the preview and in PDF→图片.
#[test]
fn renderer_draws_raw_image_xobject_through_cm() {
    use sparrowpdf::engine::render;

    let dir = scratch("raster-raw-image");
    let pdf = make_raw_image_pdf(&dir, "raw.pdf", 300.0);
    let fonts = render::FontSet::system();
    let pages =
        render::render_page_thumbnails(&pdf, None, &[1], 300.0, &fonts).expect("render image page");
    let pixmap = &pages[0].1;

    let middle = ((pixmap.height / 2) * pixmap.width + pixmap.width / 2) as usize * 4;
    let (r, g, b) = (
        pixmap.data[middle],
        pixmap.data[middle + 1],
        pixmap.data[middle + 2],
    );
    assert!(
        r < 60 && g < 60 && b > 150,
        "centre must be the blue image, got ({r},{g},{b})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A zero-page document must be rejected, not "successfully rotated".
#[test]
fn rotate_rejects_document_without_pages() {
    let dir = scratch("rotate-empty");
    let src = make_pdf(&dir, "empty.pdf", 0, 0);

    let req = request(ToolId::Rotate, vec![src], dir.join("out.pdf"));
    match run(ToolId::Rotate, &req) {
        Err(ToolError::UnsupportedFormat { detail, .. }) => {
            assert!(detail.contains("没有页面"), "detail was {detail}")
        }
        other => panic!("expected UnsupportedFormat, got {:?}", other.is_ok()),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Decrypting a file that was never encrypted must say so, and must not require
/// a password that cannot exist.
#[test]
fn decrypt_plain_file_reports_not_encrypted() {
    let dir = scratch("decrypt-plain");
    let src = make_pdf(&dir, "plain.pdf", 1, 0);

    let mut req = request(ToolId::Encrypt, vec![src], dir.join("out.pdf"));
    if let ToolOptions::Encrypt { encrypt, .. } = &mut req.options {
        *encrypt = false;
    }
    // No password supplied on purpose.
    match run(ToolId::Encrypt, &req) {
        Err(ToolError::UnsupportedFormat { detail, .. }) => {
            assert!(detail.contains("未加密"), "detail was {detail}")
        }
        other => panic!("expected UnsupportedFormat, got {:?}", other.is_ok()),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Encrypting with an empty owner password must reuse the user password instead
/// of failing (the panel documents it as optional).
#[test]
fn encrypt_accepts_empty_owner_password() {
    let dir = scratch("encrypt-owner");
    let src = make_pdf(&dir, "e2.pdf", 1, 0);
    let out = dir.join("locked2.pdf");

    let mut req = request(ToolId::Encrypt, vec![src], out.clone());
    if let ToolOptions::Encrypt {
        user_password,
        owner_password,
        ..
    } = &mut req.options
    {
        *user_password = "only-user".to_string();
        owner_password.clear();
    }
    run(ToolId::Encrypt, &req).expect("encrypt with blank owner password");
    assert!(pdf_io::is_encrypted(&out).unwrap());
    let doc = pdf_io::open_document(&out, Some("only-user")).expect("opens with user password");
    assert_eq!(pdf_io::page_count(&doc), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------- output conflicts

/// `<name>-2.pdf`, `<name>-3.pdf`, … must be what `Unique` picks.
#[test]
fn rotate_refuses_to_overwrite_without_permission() {
    let dir = scratch("conflict-report");
    let src = make_pdf(&dir, "src.pdf", 1, 0);
    let out = dir.join("rotated.pdf");

    let req = request(ToolId::Rotate, vec![src.clone()], out.clone());
    run(ToolId::Rotate, &req).expect("first run writes the file");
    let before = std::fs::read(&out).expect("read first artifact");

    // Second run with the default policy must stop before writing anything.
    match run(ToolId::Rotate, &req) {
        Err(ToolError::OutputExists { paths }) => {
            assert_eq!(paths, vec![out.clone()], "the clashing path is reported");
        }
        other => panic!("expected OutputExists, got {:?}", other.is_ok()),
    }
    assert_eq!(
        std::fs::read(&out).unwrap(),
        before,
        "a refused run must leave the existing file untouched"
    );
}

#[test]
fn rotate_unique_keeps_both_files() {
    let dir = scratch("conflict-unique");
    let src = make_pdf(&dir, "src.pdf", 1, 0);
    let out = dir.join("rotated.pdf");

    run(
        ToolId::Rotate,
        &request(ToolId::Rotate, vec![src.clone()], out.clone()),
    )
    .expect("first run");
    let before = std::fs::read(&out).unwrap();

    let mut req = request(ToolId::Rotate, vec![src], out.clone());
    req.conflict = ConflictPolicy::Unique;
    let outcome = run(ToolId::Rotate, &req).expect("unique run");
    assert_eq!(outcome.outputs.len(), 1);
    let second = &outcome.outputs[0];
    assert_eq!(second, &dir.join("rotated-2.pdf"));
    assert!(second.is_file(), "the renamed artifact exists");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        before,
        "the original artifact is preserved"
    );
}

#[test]
fn rotate_overwrite_replaces_when_asked() {
    let dir = scratch("conflict-overwrite");
    let src = make_pdf(&dir, "src.pdf", 1, 0);
    let out = dir.join("rotated.pdf");
    // A decoy with a known, wrong content: identical reruns would otherwise
    // produce identical bytes and prove nothing.
    std::fs::write(&out, b"decoy, not a pdf").unwrap();

    let mut req = request(ToolId::Rotate, vec![src], out.clone());
    req.conflict = ConflictPolicy::Overwrite;
    run(ToolId::Rotate, &req).expect("overwrite run");

    let doc = pdf_io::open_document(&out, None).expect("overwrite produced a real PDF");
    assert_eq!(pdf_io::page_count(&doc), 1);
    let page_id = *doc.get_pages().values().next().unwrap();
    assert_eq!(pdf_io::page_rotation(&doc, page_id), 90, "default +90°");
    assert!(
        !dir.join("rotated-2.pdf").exists(),
        "overwrite must not leave a renamed copy behind"
    );
}

/// A batch tool must report *every* clashing output in one go, so the dialog can
/// answer for the whole run.
#[test]
fn split_reports_every_conflict_before_writing() {
    let dir = scratch("conflict-split");
    let src = make_pdf(&dir, "doc.pdf", 4, 0);
    let out_dir = dir.join("parts");

    let mut req = request(ToolId::Split, vec![src.clone()], out_dir.clone());
    if let ToolOptions::Split { mode, every_n, .. } = &mut req.options {
        *mode = PageRangeMode::EveryN;
        *every_n = 2;
    }
    run(ToolId::Split, &req).expect("first split");

    let before: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(before.len(), 2);

    match run(ToolId::Split, &req) {
        Err(ToolError::OutputExists { paths }) => {
            assert_eq!(paths.len(), 2, "both existing parts are reported");
        }
        other => panic!("expected OutputExists, got {:?}", other.is_ok()),
    }

    // Nothing new appeared, and the existing parts are untouched.
    let after: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(after.len(), 2, "a refused batch writes nothing");
}

#[test]
fn split_unique_renames_every_output() {
    let dir = scratch("conflict-split-unique");
    let src = make_pdf(&dir, "doc.pdf", 4, 0);
    let out_dir = dir.join("parts");

    let mut req = request(ToolId::Split, vec![src], out_dir);
    if let ToolOptions::Split { mode, every_n, .. } = &mut req.options {
        *mode = PageRangeMode::EveryN;
        *every_n = 2;
    }
    run(ToolId::Split, &req).expect("first split");

    req.conflict = ConflictPolicy::Unique;
    let outcome = run(ToolId::Split, &req).expect("unique split");
    let names: Vec<String> = outcome
        .outputs
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["doc_part01-2.pdf", "doc_part02-2.pdf"]);
    for output in &outcome.outputs {
        assert_eq!(pdf_io::page_count_of(output, None).unwrap(), 2);
    }
}

// ------------------------------------------------------ image codecs

/// Deflate helper for fixtures: the codecs under test expect real streams.
fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).expect("deflate");
    encoder.finish().expect("finish")
}

/// Build a one-page PDF whose only content is `image` drawn across the page.
fn make_image_page_pdf(
    dir: &Path,
    name: &str,
    page: f32,
    image: lopdf::Dictionary,
    payload: Vec<u8>,
    content: &str,
) -> PathBuf {
    use lopdf::{dictionary, Document, Stream};

    let mut doc = Document::with_version("1.5");
    let image_id = doc.add_object(Stream::new(image, payload));
    let content_id = doc.add_object(Stream::new(Default::default(), content.as_bytes().to_vec()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), page.into(), page.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image_id } },
    });
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => 1i64,
        "Kids" => vec![page_id.into()],
    });
    if let Ok(dict) = doc.get_object_mut(page_id).unwrap().as_dict_mut() {
        dict.set("Parent", pages_id);
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

fn render_centre(path: &Path, page: f32) -> (u8, u8, u8) {
    use sparrowpdf::engine::render;
    let fonts = render::FontSet::system();
    let pages =
        render::render_page_thumbnails(path, None, &[1], page, &fonts).expect("render fixture");
    let pixmap = &pages[0].1;
    let middle = ((pixmap.height / 2) * pixmap.width + pixmap.width / 2) as usize * 4;
    (
        pixmap.data[middle],
        pixmap.data[middle + 1],
        pixmap.data[middle + 2],
    )
}

/// `/LZWDecode` must decode to the same samples as the raw image does.
#[test]
fn renderer_decodes_lzw_image() {
    use lopdf::dictionary;
    use weezl::{encode::Encoder, BitOrder};

    let dir = scratch("codec-lzw");
    let (w, h) = (8u32, 8u32);
    let mut samples = Vec::new();
    for _ in 0..(w * h) {
        samples.extend_from_slice(&[0, 0, 255]);
    }
    let mut encoder = Encoder::with_tiff_size_switch(BitOrder::Msb, 8);
    let encoded = encoder.encode(&samples).expect("lzw encode");

    let pdf = make_image_page_pdf(
        &dir,
        "lzw.pdf",
        200.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8i64,
            "Filter" => "LZWDecode",
        },
        encoded,
        "q 160 0 0 160 20 20 cm /Im1 Do Q",
    );

    let (r, g, b) = render_centre(&pdf, 200.0);
    assert!(
        r < 60 && g < 60 && b > 150,
        "expected blue, got ({r},{g},{b})"
    );
}

/// `/CCITTFaxDecode` (G3 1D here) must paint the fax raster.
#[test]
fn renderer_decodes_ccitt_image() {
    use lopdf::dictionary;

    let dir = scratch("codec-ccitt");
    // One row of eight black pixels: the reference vector from hayro-ccitt's
    // own test suite for `K 0 /Columns 8 /Rows 1`.
    let payload = vec![0x35, 0x14];
    let pdf = make_image_page_pdf(
        &dir,
        "ccitt.pdf",
        200.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 8i64,
            "Height" => 1i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 1i64,
            "Filter" => "CCITTFaxDecode",
            "DecodeParms" => dictionary! {
                "K" => 0i64,
                "Columns" => 8i64,
                "Rows" => 1i64,
                "BlackIs1" => true,
            },
        },
        payload,
        "q 160 0 0 20 20 90 cm /Im1 Do Q",
    );

    let (r, g, b) = render_centre(&pdf, 200.0);
    // The page centre is inside the drawn strip; an undecoded image leaves the
    // white page background.
    assert!(
        r < 120 && g < 120 && b < 120,
        "expected the fax row painted dark, got ({r},{g},{b})"
    );
}

/// `/BlackIs1` describes how the samples were packed, not how the page looks:
/// the same fax runs must render identically either way.
#[test]
fn renderer_ccitt_ignores_black_is_1_for_appearance() {
    use lopdf::dictionary;

    let dir = scratch("codec-ccitt-blackis1");
    let pdf = make_image_page_pdf(
        &dir,
        "ccitt-false.pdf",
        200.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 8i64,
            "Height" => 1i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 1i64,
            "Filter" => "CCITTFaxDecode",
            "DecodeParms" => dictionary! {
                "K" => 0i64,
                "Columns" => 8i64,
                "Rows" => 1i64,
                "BlackIs1" => false,
            },
        },
        vec![0x35, 0x14],
        "q 160 0 0 20 20 90 cm /Im1 Do Q",
    );

    let (r, g, b) = render_centre(&pdf, 200.0);
    assert!(
        r < 120 && g < 120 && b < 120,
        "the fax row stays dark, got ({r},{g},{b})"
    );
}

/// A PNG predictor rewrites every scanline; without undoing it the samples are
/// noise. "Up" filter with a solid colour is the cheapest honest fixture.
#[test]
fn renderer_undoes_png_up_predictor() {
    use lopdf::dictionary;

    let dir = scratch("codec-predictor");
    let (w, h) = (4usize, 4usize);
    // Every row: filter type 2 (Up) + bytes that add up to 200.
    let mut predicted = Vec::new();
    for row in 0..h {
        predicted.push(2u8);
        for _ in 0..w {
            predicted.push(if row == 0 { 200 } else { 0 });
        }
    }
    let pdf = make_image_page_pdf(
        &dir,
        "predictor.pdf",
        200.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8i64,
            "Filter" => "FlateDecode",
            "DecodeParms" => dictionary! {
                "Predictor" => 12i64,
                "Colors" => 1i64,
                "BitsPerComponent" => 8i64,
                "Columns" => w as i64,
            },
        },
        deflate(&predicted),
        "q 160 0 0 160 20 20 cm /Im1 Do Q",
    );

    let (r, _g, _b) = render_centre(&pdf, 200.0);
    assert!(
        (r as i32 - 200).abs() <= 6,
        "predictor must be undone to a flat 200 grey, got {r}"
    );
}

/// A malformed JBIG2 stream must be skipped, not crash the renderer.
#[test]
fn renderer_survives_broken_jbig2() {
    use lopdf::dictionary;

    let dir = scratch("codec-jbig2-broken");
    let pdf = make_image_page_pdf(
        &dir,
        "jbig2.pdf",
        100.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 64i64,
            "Height" => 64i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 1i64,
            "Filter" => "JBIG2Decode",
        },
        vec![0u8; 32],
        "q 80 0 0 80 10 10 cm /Im1 Do Q",
    );

    let fonts = sparrowpdf::engine::render::FontSet::system();
    let pages = sparrowpdf::engine::render::render_page_thumbnails(&pdf, None, &[1], 100.0, &fonts)
        .expect("page still renders");
    // The page stays white: the image was dropped, nothing panicked.
    let pixmap = &pages[0].1;
    let middle = ((pixmap.height / 2) * pixmap.width + pixmap.width / 2) as usize * 4;
    assert_eq!(pixmap.data[middle], 255, "undecodable image is skipped");
}

/// CID-keyed text carries glyph ids, not characters. Without `/ToUnicode` the
/// bytes used to be read as Latin-1, so a Chinese or subset-font page rendered
/// as nothing at all.
#[test]
fn renderer_reads_cid_text_through_to_unicode() {
    use lopdf::{dictionary, Document, Stream};

    let dir = scratch("text-to-unicode");
    let mut doc = Document::with_version("1.5");
    let cmap = b"/CIDInit /ProcSet findresource begin\n\
12 dict begin\nbegincmap\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
2 beginbfchar\n<0001> <0041>\n<0002> <0042>\nendbfchar\n\
endcmap\nend\nend\n";
    let cmap_id = doc.add_object(Stream::new(Default::default(), cmap.to_vec()));
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "SimSun",
        "Encoding" => "Identity-H",
        "ToUnicode" => cmap_id,
    });
    // "AB" as two-byte codes 0x0001 0x0002.
    let content_id = doc.add_object(Stream::new(
        Default::default(),
        b"BT /F1 72 Tf 1 0 0 1 40 100 Tm <00010002> Tj ET".to_vec(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => 1i64,
        "Kids" => vec![page_id.into()],
    });
    if let Ok(dict) = doc.get_object_mut(page_id).unwrap().as_dict_mut() {
        dict.set("Parent", pages_id);
    }
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.max_id = doc.objects.len() as u32;
    let path = dir.join("cid.pdf");
    doc.save(&path).expect("save fixture");

    let fonts = sparrowpdf::engine::render::FontSet::system();
    let pages =
        sparrowpdf::engine::render::render_page_thumbnails(&path, None, &[1], 200.0, &fonts)
            .expect("render page");
    let pixmap = &pages[0].1;
    let dark = pixmap
        .data
        .chunks_exact(4)
        .filter(|p| p[0] < 128 && p[3] > 0)
        .count();
    assert!(
        dark > 50,
        "two 72 pt glyphs must leave dark pixels, got {dark}"
    );
}

// ------------------------------------------------------ embedded fonts / JBIG2

/// A TrueType file to embed. The tests that need one skip themselves when the
/// host ships none (a bare CI container), because the code path under test is
/// "read the program from the PDF", not "find a system font".
fn system_truetype() -> Option<Vec<u8>> {
    const CANDIDATES: &[&str] = &[
        r"C:/Windows/Fonts/arial.ttf",
        r"C:/Windows/Fonts/segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    CANDIDATES.iter().find_map(|path| std::fs::read(path).ok())
}

/// A CID font with an embedded program and **no** `/ToUnicode`: the codes are
/// glyph indices, which only the embedded font can resolve. This is the case
/// that used to render as an empty page.
#[test]
fn renderer_draws_embedded_cid_font_without_to_unicode() {
    use lopdf::{dictionary, Document, Stream};

    let Some(program) = system_truetype() else {
        eprintln!("跳过：本机没有可嵌入的 TrueType 字体");
        return;
    };

    let dir = scratch("font-embedded-cid");
    let mut doc = Document::with_version("1.5");
    let file_id = doc.add_object(Stream::new(
        dictionary! { "Length1" => program.len() as i64 },
        program,
    ));
    let descriptor_id = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "Embedded",
        "Flags" => 32i64,
        "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 900.into()],
        "ItalicAngle" => 0i64,
        "Ascent" => 900i64,
        "Descent" => -200i64,
        "CapHeight" => 700i64,
        "StemV" => 80i64,
        "FontFile2" => file_id,
    });
    // CID 1 -> glyph 100: an arbitrary mid-font glyph, which is guaranteed to
    // have ink in any real text face.
    let cid_map_id = doc.add_object(Stream::new(Default::default(), vec![0u8, 0, 0, 100]));
    let descendant_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "Embedded",
        "CIDSystemInfo" => dictionary! {
            "Registry" => lopdf::Object::string_literal("Adobe"),
            "Ordering" => lopdf::Object::string_literal("Identity"),
            "Supplement" => 0i64,
        },
        "FontDescriptor" => descriptor_id,
        "CIDToGIDMap" => cid_map_id,
    });
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "Embedded",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![descendant_id.into()],
    });
    let content_id = doc.add_object(Stream::new(
        Default::default(),
        b"BT /F1 96 Tf 1 0 0 1 30 80 Tm <0001> Tj ET".to_vec(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => 1i64,
        "Kids" => vec![page_id.into()],
    });
    if let Ok(dict) = doc.get_object_mut(page_id).unwrap().as_dict_mut() {
        dict.set("Parent", pages_id);
    }
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.max_id = doc.objects.len() as u32;
    let path = dir.join("embedded.pdf");
    doc.save(&path).expect("save fixture");

    let fonts = sparrowpdf::engine::render::FontSet::system();
    let pages =
        sparrowpdf::engine::render::render_page_thumbnails(&path, None, &[1], 200.0, &fonts)
            .expect("render page");
    let dark = pages[0]
        .1
        .data
        .chunks_exact(4)
        .filter(|pixel| pixel[0] < 128)
        .count();
    // Without the embedded program the glyph never appears: the codepoint map
    // is empty and CID 1 means nothing to a substituted system font.
    assert!(dark > 50, "内嵌字体必须画出字形，实际深色像素 {dark}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A minimal *valid* JBIG2 stream: page information + one MMR-coded generic
/// region of four black pixels. Hand-built because the decoder has no sample
/// files of its own, and a broken stream is not the same test.
fn jbig2_stream() -> Vec<u8> {
    fn segment(number: u32, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&number.to_be_bytes());
        out.push(kind);
        out.push(0x00); // no referred-to segments
        out.push(0x01); // associated with page 1
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    let mut page_info = Vec::new();
    page_info.extend_from_slice(&4u32.to_be_bytes()); // width
    page_info.extend_from_slice(&1u32.to_be_bytes()); // height
    page_info.extend_from_slice(&0u32.to_be_bytes()); // x resolution: unknown
    page_info.extend_from_slice(&0u32.to_be_bytes()); // y resolution: unknown
    page_info.push(0); // default pixel white, combination = OR
    page_info.extend_from_slice(&0u16.to_be_bytes()); // striping

    let mut region = Vec::new();
    region.extend_from_slice(&4u32.to_be_bytes()); // region width
    region.extend_from_slice(&1u32.to_be_bytes()); // region height
    region.extend_from_slice(&0u32.to_be_bytes()); // x
    region.extend_from_slice(&0u32.to_be_bytes()); // y
    region.push(0); // combination = OR
    region.push(0x01); // MMR
                       // Horizontal mode (001), white run of 0 (00110101), black run of 4 (011).
    region.extend_from_slice(&[0x26, 0xAC]);
    region.extend_from_slice(&[0x00, 0x10, 0x01]); // EOFB

    let mut data = segment(0, 0x30, &page_info);
    data.extend(segment(1, 0x27, &region));
    data.extend(segment(2, 0x31, &[])); // end of page
    data.extend(segment(3, 0x33, &[])); // end of file
    data
}

#[test]
fn renderer_decodes_valid_jbig2_image() {
    use lopdf::dictionary;

    let dir = scratch("codec-jbig2-valid");
    let pdf = make_image_page_pdf(
        &dir,
        "jbig2-valid.pdf",
        200.0,
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 4i64,
            "Height" => 1i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 1i64,
            "Filter" => "JBIG2Decode",
        },
        jbig2_stream(),
        "q 160 0 0 40 20 80 cm /Im1 Do Q",
    );

    let (r, g, b) = render_centre(&pdf, 200.0);
    assert!(
        r < 120 && g < 120 && b < 120,
        "JBIG2 区域应为黑色，实际 ({r},{g},{b})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The write path must re-check the target: `resolve_outputs` runs before a
/// long render, and a file created in between must not be replaced.
#[test]
fn save_refuses_target_created_after_planning() {
    let dir = scratch("save-recheck");
    let target = dir.join("late.pdf");
    std::fs::write(&target, b"appeared after the plan").expect("decoy");

    let mut doc = pdf_io::new_document();
    match pdf_io::save_document(&mut doc, &target, ConflictPolicy::Report) {
        Err(ToolError::OutputExists { paths }) => assert_eq!(paths, vec![target.clone()]),
        other => panic!("expected OutputExists, got {:?}", other.is_ok()),
    }
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"appeared after the plan",
        "被拒绝的写入不得改动已有文件"
    );

    // The same call with an explicit policy goes through.
    pdf_io::save_document(&mut doc, &target, ConflictPolicy::Overwrite).expect("overwrite");
    assert!(
        pdf_io::open_document(&target, None).is_ok(),
        "覆盖后是有效 PDF"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Merge is the multi-input single-output case: the clash must be reported
/// before the pages are copied, and the existing file must survive untouched.
#[test]
fn merge_reports_existing_target_without_writing() {
    let dir = scratch("conflict-merge");
    let a = make_pdf(&dir, "a.pdf", 1, 0);
    let b = make_pdf(&dir, "b.pdf", 1, 0);
    let out = dir.join("merged.pdf");
    std::fs::write(&out, b"keep me").expect("decoy");

    let req = request(ToolId::Merge, vec![a, b], out.clone());
    match run(ToolId::Merge, &req) {
        Err(ToolError::OutputExists { paths }) => assert_eq!(paths, vec![out.clone()]),
        other => panic!("expected OutputExists, got {:?}", other.is_ok()),
    }
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let _ = std::fs::remove_dir_all(&dir);
}

/// PDF→图片 writes through `write_file` rather than `save_document`, and its
/// outputs are planned from a directory: both have to honour the policy.
#[test]
fn pdf_to_image_reports_existing_page_files() {
    let dir = scratch("conflict-images");
    let src = make_pdf(&dir, "doc.pdf", 1, 0);
    let out_dir = dir.join("images");
    std::fs::create_dir_all(&out_dir).unwrap();
    let clash = out_dir.join("doc_p001.png");
    std::fs::write(&clash, b"existing image").expect("decoy");

    let req = request(ToolId::PdfToImage, vec![src], out_dir.clone());
    match run(ToolId::PdfToImage, &req) {
        Err(ToolError::OutputExists { paths }) => assert_eq!(paths, vec![clash.clone()]),
        other => panic!("expected OutputExists, got {:?}", other.is_ok()),
    }
    assert_eq!(std::fs::read(&clash).unwrap(), b"existing image");
    // A refused run leaves nothing else behind either.
    assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
