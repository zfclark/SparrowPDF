//! Output verification.
//!
//! Every tool must call the matching `assert_*` function on its own artifact
//! before reporting success. A failed check deletes the artifact and returns
//! [`ToolError::ValidationFailed`].

use std::path::Path;

use lopdf::Object;

use crate::engine::pdf_io;
use crate::model::ToolError;

/// Assert a PDF exists, re-opens, and has exactly `expected_pages` pages.
///
/// `password` must be supplied when the artifact is encrypted: lopdf opens an
/// encrypted document without credentials but yields zero pages, so a
/// password-less reopen would fail every page-count check.
pub fn assert_page_count(
    path: &Path,
    expected_pages: u32,
    password: Option<&str>,
) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    let actual = if doc.is_encrypted() {
        let Some(pw) = password else {
            return fail(path, "产物已加密但未提供校验密码，无法核对页数".to_string());
        };
        let bytes = std::fs::read(path).map_err(|e| ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
        let opened = lopdf::Document::load_mem_with_password(&bytes, pw).map_err(|e| {
            fail::<lopdf::Document>(path, format!("产物无法用校验密码打开：{e}"))
                .err()
                .unwrap()
        })?;
        pdf_io::page_count(&opened)
    } else {
        pdf_io::page_count(&doc)
    };
    if actual != expected_pages {
        return fail(
            path,
            format!("页数不符：期望 {expected_pages}，实际 {actual}"),
        );
    }
    Ok(())
}

/// Assert every page of the artifact shares one orientation.
///
/// Image→PDF layouts fix the page orientation once per document, so a mixed
/// result is always a defect in the page loop (typically a page type that
/// lost the shared policy) rather than user intent. `expect_landscape` is
/// `true` for A4 landscape, `false` for A4 portrait.
pub fn assert_page_orientation(path: &Path, expect_landscape: bool) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    for (page_num, page_id) in doc.get_pages() {
        let [x0, y0, x1, y1] = pdf_io::page_media_box(&doc, page_id);
        let is_landscape = (x1 - x0) > (y1 - y0);
        if is_landscape != expect_landscape {
            let want = if expect_landscape { "横向" } else { "纵向" };
            let got = if is_landscape { "横向" } else { "纵向" };
            return fail(
                path,
                format!("第 {page_num} 页方向与全书不一致：期望{want}，实际{got}"),
            );
        }
    }
    Ok(())
}

/// Assert the `/Rotate` value of every listed page.
///
/// `expected` holds `(1-based page number, degrees)` pairs.
pub fn assert_rotations(path: &Path, expected: &[(u32, u32)]) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    let pages = doc.get_pages();
    for (page_num, degrees) in expected {
        let Some(page_id) = pages.get(page_num).copied() else {
            return fail(path, format!("第 {page_num} 页不存在"));
        };
        let actual = pdf_io::page_rotation(&doc, page_id);
        if actual != *degrees {
            return fail(
                path,
                format!("第 {page_num} 页旋转值不符：期望 {degrees}，实际 {actual}"),
            );
        }
    }
    Ok(())
}

/// Assert the document is encrypted (i.e. still carries an `/Encrypt` entry).
pub fn assert_encrypted(path: &Path) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    if !doc.is_encrypted() {
        return fail(path, "产物未加密".to_string());
    }
    Ok(())
}

/// Assert the document is not encrypted.
pub fn assert_decrypted(path: &Path) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    if doc.is_encrypted() {
        return fail(path, "产物仍处于加密状态".to_string());
    }
    Ok(())
}

/// Assert that `expected` DCTDecode image XObjects exist.
///
/// This is the invariant behind "JPEG must be embedded as DCTDecode without
/// re-encoding": if the count is short, something was re-encoded as Flate.
pub fn assert_dctdecode_count(path: &Path, expected: usize) -> Result<(), ToolError> {
    let doc = reopen(path)?;
    let mut found = 0usize;
    for object in doc.objects.values() {
        let Object::Stream(stream) = object else {
            continue;
        };
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok());
        if subtype != Some(b"Image".as_slice()) {
            continue;
        }
        match stream.dict.get(b"Filter") {
            Ok(Object::Name(name)) if name == b"DCTDecode" => found += 1,
            Ok(Object::Array(filters)) => {
                let has_dct = filters
                    .iter()
                    .any(|f| matches!(f, Object::Name(n) if n == b"DCTDecode"));
                if has_dct {
                    found += 1;
                }
            }
            _ => {}
        }
    }
    if found != expected {
        return fail(
            path,
            format!("DCTDecode 图像数量不符：期望 {expected}，实际 {found}"),
        );
    }
    Ok(())
}

/// Assert the raw file bytes still contain `expected` JPEG SOI markers
/// (`FF D8 FF`). Used as a second, lossless check next to
/// [`assert_dctdecode_count`].
pub fn assert_jpeg_soi_bytes(path: &Path, min_count: usize) -> Result<(), ToolError> {
    let bytes = std::fs::read(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    let mut count = 0usize;
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] == 0xFF && bytes[i + 1] == 0xD8 && bytes[i + 2] == 0xFF {
            count += 1;
            i += 3;
        } else {
            i += 1;
        }
    }
    if count < min_count {
        return Err(ToolError::ValidationFailed {
            detail: format!("JPEG 数据未原样保留：期望至少 {min_count} 段，实际 {count} 段"),
        });
    }
    Ok(())
}

/// Assert a produced text file is non-empty (ignoring whitespace).
///
/// Only used by the (removed) text-extraction tool today; kept for the
/// fixed validation-helper shape in case text output returns.
#[allow(dead_code)]
pub fn assert_non_empty_text(path: &Path) -> Result<(), ToolError> {
    let text = std::fs::read_to_string(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    if text.trim().is_empty() {
        return fail(path, "文本内容为空".to_string());
    }
    Ok(())
}

/// Assert a produced image file opens and has the expected dimensions.
pub fn assert_image(path: &Path, width: u32, height: u32) -> Result<(), ToolError> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?
        .with_guessed_format()
        .map_err(|e| ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| ToolError::ValidationFailed {
            detail: format!("无法读取产物图片尺寸：{e}"),
        })?;
    if w != width || h != height {
        return fail(
            path,
            format!("图片尺寸不符：期望 {width}x{height}，实际 {w}x{h}"),
        );
    }
    Ok(())
}

/// Assert two documents have identical page counts (used by merge).
pub fn assert_merged_page_total(
    path: &Path,
    source_page_totals: &[u32],
    password: Option<&str>,
) -> Result<(), ToolError> {
    let expected: u32 = source_page_totals.iter().sum();
    assert_page_count(path, expected, password)
}

fn reopen(path: &Path) -> Result<lopdf::Document, ToolError> {
    if !path.exists() {
        return Err(ToolError::ValidationFailed {
            detail: format!("产物不存在：{}", path.display()),
        });
    }
    lopdf::Document::load(path).map_err(|e| ToolError::ValidationFailed {
        detail: format!("产物无法重新打开：{e}"),
    })
}

/// Delete the artifact and return a validation error.
fn fail<T>(path: &Path, detail: String) -> Result<T, ToolError> {
    pdf_io::remove_quietly(path);
    Err(ToolError::ValidationFailed {
        detail: format!("{}：{detail}", path.display()),
    })
}
