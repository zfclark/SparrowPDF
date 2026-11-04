//! Tool 1: merge images into a PDF.
//!
//! Image embedding, EXIF handling and page layout live in
//! [`crate::tools::image_embed`], shared with the folder batch tool.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lopdf::Document;

use crate::engine::{pdf_io, validate};
use crate::model::{InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::image_embed;
use crate::tools::{check_cancel, CancelFlag, Progress, Tool, ToolOptions, ToolRequest};

pub struct ImageToPdf;

impl Tool for ImageToPdf {
    fn id(&self) -> ToolId {
        ToolId::ImageToPdf
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        if req.inputs.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请先添加至少一张 JPEG 或 PNG 图片".to_string(),
            });
        }
        for input in &req.inputs {
            let ext = extension_of(&input.path)?;
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                return Err(ToolError::UnsupportedFormat {
                    path: Some(input.path.clone()),
                    detail: format!("仅支持 JPEG / PNG，收到 .{ext}"),
                });
            }
            if !input.path.is_file() {
                return Err(ToolError::ReadFailed {
                    path: input.path.clone(),
                    detail: "文件不存在".to_string(),
                });
            }
        }
        Ok(())
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let images = dedupe(req.inputs.clone());
        if images.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "没有可用的图片".to_string(),
            });
        }

        let (force, auto_rotate) = match &req.options {
            ToolOptions::ImageToPdf {
                force_orientation,
                auto_rotate,
            } => (*force_orientation, *auto_rotate),
            // No silent fallback: every other tool rejects a mismatched
            // option set, and swallowing it here would quietly drop the
            // orientation settings.
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };
        // One layout for the whole document: every image page is written with
        // the same orientation policy and margin.
        let layout = image_embed::ImagePageLayout {
            orientation: force,
            auto_rotate,
            margin: image_embed::PAGE_MARGIN,
        };

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        let mut doc = Document::with_version("1.7");
        let pages_id = build_document_skeleton(&mut doc);

        let total = images.len();
        let mut jpeg_count = 0usize;
        let mut skipped: Vec<(PathBuf, ToolError)> = Vec::new();

        for (index, input) in images.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("处理图片 {}/{}：{}", index + 1, total, input.display_name()),
            );

            match image_embed::add_image_page(&mut doc, pages_id, &input.path, layout, index) {
                Ok(is_jpeg) => {
                    if is_jpeg {
                        jpeg_count += 1;
                    }
                }
                Err(err) => {
                    // One bad image must not abort the batch; record and continue.
                    skipped.push((input.path.clone(), err));
                }
            }
        }

        if page_count_in(&doc) == 0 {
            let detail = skipped
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有任何图片可用（{detail}）"),
            });
        }

        progress.report(0.95, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut doc, &out, req.conflict)?;

        let expected_pages = page_count_in(&doc);
        validate::assert_page_count(&out, expected_pages, None)?;
        if jpeg_count > 0 {
            validate::assert_dctdecode_count(&out, jpeg_count)?;
            // Raw SOI markers must still be present, proving no re-encode happened.
            validate::assert_jpeg_soi_bytes(&out, jpeg_count)?;
        }

        progress.report(1.0, "完成".to_string());

        let mut summary = format!("已生成 {expected_pages} 页 PDF（JPEG 直通 {jpeg_count} 张）");
        if !skipped.is_empty() {
            summary.push_str(&format!("，跳过 {} 张", skipped.len()));
            return Err(ToolError::PartialSuccess {
                succeeded: expected_pages as usize,
                failed: skipped.len(),
                detail: format!("{summary}；{}", describe_skips(&skipped)),
                outputs: vec![out.clone()],
            });
        }

        Ok(ToolOutcome::success(summary, vec![out]))
    }
}

/// Remove duplicate inputs while preserving order (canonicalised where possible).
fn dedupe(inputs: Vec<InputFile>) -> Vec<InputFile> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::with_capacity(inputs.len());
    for input in inputs {
        let key = input
            .path
            .canonicalize()
            .unwrap_or_else(|_| input.path.clone());
        if seen.insert(key) {
            out.push(input);
        }
    }
    out
}

fn describe_skips(skipped: &[(PathBuf, ToolError)]) -> String {
    skipped
        .iter()
        .map(|(path, err)| format!("{}: {err}", path.display()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn extension_of(path: &Path) -> Result<String, ToolError> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .ok_or_else(|| ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "文件没有扩展名".to_string(),
        })
}

/// Minimal but valid single-page-tree document.
fn build_document_skeleton(doc: &mut Document) -> lopdf::ObjectId {
    let pages_id = doc.new_object_id();
    use lopdf::{dictionary, Object};
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => Vec::<Object>::new(),
            "Count" => 0i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    pages_id
}

fn page_count_in(doc: &Document) -> u32 {
    doc.get_pages().len() as u32
}
