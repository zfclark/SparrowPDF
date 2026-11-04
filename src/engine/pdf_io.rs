//! PDF open/save plumbing shared by every tool.
//!
//! All fallible operations map onto [`ToolError`] here so individual tools never
//! have to translate `lopdf`/`medpdf` errors themselves.

use std::fs;
use std::path::{Path, PathBuf};

use lopdf::dictionary;
use lopdf::Document;

use crate::model::{ConflictPolicy, ToolError};

/// Open a PDF, converting every library error into a [`ToolError`].
///
/// Encrypted documents are detected before decryption is attempted so the UI can
/// distinguish "needs a password" from "the password is wrong":
///
/// * no password supplied and the file is encrypted -> [`ToolError::PasswordRequired`]
/// * password supplied but rejected -> [`ToolError::WrongPassword`]
pub fn open_document(path: &Path, password: Option<&str>) -> Result<Document, ToolError> {
    if !path.exists() {
        return Err(ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: "文件不存在".to_string(),
        });
    }

    // First probe: does the file even parse? This also tells us whether it is
    // encrypted without needing a password.
    let probe = Document::load(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;

    if !probe.is_encrypted() {
        return Ok(probe);
    }

    let Some(password) = password else {
        return Err(ToolError::PasswordRequired {
            path: path.to_path_buf(),
        });
    };

    // Re-open with the password so lopdf decrypts object streams during the read.
    let mut doc =
        Document::load_with_password(path, password).map_err(|_| ToolError::WrongPassword {
            path: path.to_path_buf(),
        })?;

    // Belt and braces: if the trailer still references /Encrypt, the password was
    // accepted by the handler but the content was not fully decrypted.
    if doc.is_encrypted() {
        doc.decrypt(password)
            .map_err(|_| ToolError::WrongPassword {
                path: path.to_path_buf(),
            })?;
    }

    Ok(doc)
}

/// Whether the file declares `/Encrypt` on disk.
///
/// Answering this needs no password (`lopdf` reads the trailer but cannot
/// decrypt object streams), which is what lets the decrypt tool say "this file
/// is not encrypted" instead of demanding a password that cannot exist.
pub fn is_encrypted(path: &Path) -> Result<bool, ToolError> {
    let doc = Document::load(path).map_err(|e| ToolError::ReadFailed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    Ok(doc.is_encrypted())
}

/// Open a PDF that is expected to be a normal, unencrypted working document.
///
/// Encrypted inputs still open fine here as long as a password is provided, but
/// the caller gets [`ToolError::PasswordRequired`] when it is missing.
pub fn open_for_edit(path: &Path, password: Option<&str>) -> Result<Document, ToolError> {
    open_document(path, password)
}

/// Human-facing detail for a write error.
///
/// Permission problems are the common "cannot save the output" case when the
/// app runs without administrator rights (protected folders such as
/// `C:\Program Files`, another user's profile, or a file currently locked by
/// a PDF viewer), so call both out explicitly instead of surfacing a raw
/// OS message.
pub(crate) fn write_error_detail(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "没有写入权限：目录可能受系统保护（如 Program Files），或文件正被其他程序（如 PDF 阅读器）占用。请关闭占用该文件的程序；若目录受保护，请在「输出」中点击「选择输出文件…」改存到「文档」或桌面。".to_string()
        }
        _ => err.to_string(),
    }
}

/// Probe whether `dir` accepts file creation, so the UI can fall back to a
/// writable folder instead of failing at save time. The probe file is removed
/// immediately; a `false` answer means the process could not write there.
pub fn dir_is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(".sparrow_write_probe");
    match fs::File::create(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Save `doc` to `path` through a sibling temp file, so a failed write never
/// leaves a half-written artifact behind.
pub fn save_document(
    doc: &mut Document,
    path: &Path,
    policy: ConflictPolicy,
) -> Result<(), ToolError> {
    ensure_still_free(path, policy)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| ToolError::WriteFailed {
                path: parent.to_path_buf(),
                detail: write_error_detail(&e),
            })?;
        }
    }

    let tmp = temp_sibling(path);
    doc.save(&tmp).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        ToolError::WriteFailed {
            path: path.to_path_buf(),
            detail: write_error_detail(&e),
        }
    })?;

    // `doc.save` wrote to the temp path; move it into place.
    fs::rename(&tmp, path)
        .or_else(|_| {
            // Windows can refuse a cross-device rename or an existing target.
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path)
        })
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            ToolError::WriteFailed {
                path: path.to_path_buf(),
                detail: write_error_detail(&e),
            }
        })?;

    Ok(())
}

/// A temp path next to `path`, so the final rename stays on the same volume.
fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output.pdf".to_string());
    path.with_file_name(format!(".{name}.sparrowtmp"))
}

/// Number of pages, as the page tree reports it.
pub fn page_count(doc: &Document) -> u32 {
    doc.get_pages().len() as u32
}

/// Page count of a file on disk, without handing a `Document` to the caller.
///
/// This is the only page-count entry point the UI layer may use, so no `lopdf`
/// type crosses into `ui`.
pub fn page_count_of(path: &Path, password: Option<&str>) -> Result<u32, ToolError> {
    let doc = open_document(path, password)?;
    Ok(page_count(&doc))
}

/// Write a file to disk with a mapped error.
pub fn write_file(path: &Path, bytes: &[u8], policy: ConflictPolicy) -> Result<(), ToolError> {
    ensure_still_free(path, policy)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| ToolError::WriteFailed {
                path: parent.to_path_buf(),
                detail: write_error_detail(&e),
            })?;
        }
    }
    fs::write(path, bytes).map_err(|e| ToolError::WriteFailed {
        path: path.to_path_buf(),
        detail: write_error_detail(&e),
    })
}

/// Parse `1-3,7` style page specs into sorted, 1-based, de-duplicated pages.
///
/// Unlike `medpdf::parse_page_spec`, out-of-range pages are an error rather than
/// being silently dropped, because silently producing a 3-page file from a
/// 5-page request would break the output assertion.
pub fn parse_pages(spec: &str, total: u32) -> Result<Vec<u32>, ToolError> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidPageRange {
            detail: "页码范围为空".to_string(),
        });
    }
    if trimmed.eq_ignore_ascii_case("all") || trimmed == "全部" {
        return Ok((1..=total).collect());
    }

    let mut pages: Vec<u32> = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => {
                let start = parse_page_number(a, total)?;
                let end = if b.trim().is_empty() {
                    total
                } else {
                    parse_page_number(b, total)?
                };
                (start, end)
            }
            None => {
                let n = parse_page_number(part, total)?;
                (n, n)
            }
        };
        if start > end {
            return Err(ToolError::InvalidPageRange {
                detail: format!("起始页 {start} 大于结束页 {end}"),
            });
        }
        for n in start..=end {
            if !pages.contains(&n) {
                pages.push(n);
            }
        }
    }

    if pages.is_empty() {
        return Err(ToolError::InvalidPageRange {
            detail: format!("无法从「{spec}」解析出任何页码"),
        });
    }
    pages.sort_unstable();
    Ok(pages)
}

fn parse_page_number(raw: &str, total: u32) -> Result<u32, ToolError> {
    let n: u32 = raw
        .trim()
        .parse()
        .map_err(|_| ToolError::InvalidPageRange {
            detail: format!("「{}」不是合法页码", raw.trim()),
        })?;
    if n == 0 {
        return Err(ToolError::InvalidPageRange {
            detail: "页码从 1 开始".to_string(),
        });
    }
    if total > 0 && n > total {
        return Err(ToolError::InvalidPageRange {
            detail: format!("页码 {n} 超出总页数 {total}"),
        });
    }
    Ok(n)
}

/// Current `/Rotate` of a page, normalized to 0/90/180/270.
pub fn page_rotation(doc: &Document, page_id: lopdf::ObjectId) -> u32 {
    medpdf::get_page_rotation(doc, page_id)
}

/// Media box of a page as `[x0, y0, x1, y1]`, defaulting to A4.
pub fn page_media_box(doc: &Document, page_id: lopdf::ObjectId) -> [f32; 4] {
    medpdf::get_page_media_box(doc, page_id).unwrap_or([0.0, 0.0, 595.0, 842.0])
}

/// Decide the final path of every planned output before anything is written.
///
/// `Report` returns every clashing path at once (not just the first) so the UI
/// can say "3 个文件已存在" and offer a single decision for the whole run.
/// `Unique` keeps the previous files and picks `name-2`, `name-3`, …; names
/// reserved by the same run are skipped too, so two planned files can never be
/// renamed onto each other.
pub fn resolve_outputs(
    planned: &[std::path::PathBuf],
    policy: ConflictPolicy,
) -> Result<Vec<std::path::PathBuf>, ToolError> {
    match policy {
        ConflictPolicy::Report => {
            let existing: Vec<_> = planned.iter().filter(|p| p.exists()).cloned().collect();
            if existing.is_empty() {
                Ok(planned.to_vec())
            } else {
                Err(ToolError::OutputExists { paths: existing })
            }
        }
        ConflictPolicy::Overwrite => Ok(planned.to_vec()),
        ConflictPolicy::Unique => {
            let mut taken: std::collections::BTreeSet<std::path::PathBuf> =
                planned.iter().cloned().collect();
            let mut resolved = Vec::with_capacity(planned.len());
            for path in planned {
                resolved.push(unique_path(path, &mut taken)?);
            }
            Ok(resolved)
        }
    }
}

/// [`resolve_outputs`] for a tool that writes exactly one file.
pub fn resolve_output(path: &Path, policy: ConflictPolicy) -> Result<PathBuf, ToolError> {
    let mut resolved = resolve_outputs(std::slice::from_ref(&path.to_path_buf()), policy)?;
    Ok(resolved.remove(0))
}

/// First free `<stem>-N.<ext>` next to `path`.
///
/// Running out of serials is an error, not a fallback: returning the original
/// path would overwrite the very file the caller asked to preserve.
fn unique_path(
    path: &Path,
    taken: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<PathBuf, ToolError> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    for serial in 2..10_000u32 {
        let name = if ext.is_empty() {
            format!("{stem}-{serial}")
        } else {
            format!("{stem}-{serial}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() && !taken.contains(&candidate) {
            taken.insert(candidate.clone());
            return Ok(candidate);
        }
    }
    Err(ToolError::OutputNameExhausted {
        path: path.to_path_buf(),
    })
}

/// Refuse a path that already exists unless the caller asked to overwrite.
///
/// `resolve_outputs` runs before a tool starts rendering, which can take
/// minutes; the file could appear in the meantime (another instance, another
/// program). Re-checking right before the rename closes that window to the
/// microseconds between the check and the move.
fn ensure_still_free(path: &Path, policy: ConflictPolicy) -> Result<(), ToolError> {
    if policy == ConflictPolicy::Report && path.exists() {
        return Err(ToolError::OutputExists {
            paths: vec![path.to_path_buf()],
        });
    }
    Ok(())
}

/// Delete a file if it exists, ignoring errors (used when verification fails).
pub fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Create an empty document with a valid catalog and a single `/Pages` node.
pub fn new_document() -> Document {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    doc.objects.insert(
        pages_id,
        lopdf::Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => Vec::<lopdf::Object>::new(),
            "Count" => 0i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc
}

/// Copy the listed 1-based pages of `source` into `dest`, preserving order.
///
/// `medpdf::copy_page_with_cache` deep-copies every referenced object, shares
/// resources across calls, re-parents each new page onto `dest`'s `/Pages` node
/// and appends it to `/Kids`, so the insertion order is the output order.
pub fn copy_pages(dest: &mut Document, source: &Document, pages: &[u32]) -> Result<(), ToolError> {
    let mut cache: std::collections::BTreeMap<lopdf::ObjectId, lopdf::ObjectId> =
        std::collections::BTreeMap::new();
    for page in pages {
        medpdf::copy_page_with_cache(dest, source, *page, &mut cache).map_err(|e| {
            ToolError::Internal {
                detail: format!("复制第 {page} 页失败：{e}"),
            }
        })?;
    }
    Ok(())
}
