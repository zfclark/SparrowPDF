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
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
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
    assert_eq!(all.len(), 12, "spec fixes exactly 12 tools");
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            assert_ne!(all[i], all[j]);
        }
    }
    for id in all {
        assert!(!id.label().is_empty());
    }
}
