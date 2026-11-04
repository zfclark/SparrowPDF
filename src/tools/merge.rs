//! Tool 2: merge several PDFs in list order.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lopdf::dictionary;
use lopdf::{Document, ObjectId};

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{check_cancel, CancelFlag, Progress, Tool, ToolRequest};

pub struct Merge;

impl Tool for Merge {
    fn id(&self) -> ToolId {
        ToolId::Merge
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        if req.inputs.len() < 2 {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "PDF 合并至少需要 2 个文件".to_string(),
            });
        }
        for input in &req.inputs {
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
        // Claim the output before doing the work: the user should not wait for
        // a 200-page merge to be asked whether to overwrite a file.
        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        let mut dest = Document::with_version("1.7");
        new_skeleton(&mut dest);

        let total = req.inputs.len();
        let mut succeeded: Vec<u32> = Vec::new();
        let mut skipped: Vec<(PathBuf, ToolError)> = Vec::new();

        for (index, input) in req.inputs.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("合并 {}/{}：{}", index + 1, total, input.display_name()),
            );

            let source = match pdf_io::open_document(&input.path, req.password.as_deref()) {
                Ok(doc) => doc,
                Err(err) => {
                    skipped.push((input.path.clone(), err));
                    continue;
                }
            };

            let page_total = pdf_io::page_count(&source);
            if page_total == 0 {
                skipped.push((
                    input.path.clone(),
                    ToolError::UnsupportedFormat {
                        path: Some(input.path.clone()),
                        detail: "文档没有页面".to_string(),
                    },
                ));
                continue;
            }

            if let Err(err) = append_document(&mut dest, &source, page_total) {
                skipped.push((input.path.clone(), err));
                continue;
            }
            succeeded.push(page_total);
        }

        if succeeded.is_empty() {
            return Err(ToolError::PartialSuccess {
                succeeded: 0,
                failed: skipped.len(),
                detail: describe(&skipped),
                outputs: Vec::new(),
            });
        }

        progress.report(0.95, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut dest, &out, req.conflict)?;

        let expected: u32 = succeeded.iter().sum();
        validate::assert_merged_page_total(&out, &succeeded, None)?;

        progress.report(1.0, "完成".to_string());

        if !skipped.is_empty() {
            return Err(ToolError::PartialSuccess {
                succeeded: succeeded.len(),
                failed: skipped.len(),
                detail: format!(
                    "已合并 {expected} 页，{} 个文件失败：{}",
                    skipped.len(),
                    describe(&skipped)
                ),
                outputs: vec![out.clone()],
            });
        }

        Ok(ToolOutcome::success(
            format!("已合并 {} 个文件，共 {expected} 页", succeeded.len()),
            vec![out],
        ))
    }
}

fn describe(skipped: &[(PathBuf, ToolError)]) -> String {
    skipped
        .iter()
        .map(|(p, e)| format!("{}: {e}", p.display()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Empty document skeleton (single `/Pages` node).
fn new_skeleton(doc: &mut Document) -> ObjectId {
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
    pages_id
}

/// Copy every page of `source` into `dest` in order, sharing a resource cache.
///
/// `medpdf::copy_page_with_cache` already deep-copies referenced objects,
/// re-parents the new page onto our `/Pages` node, and appends it to `/Kids`,
/// so insertion order here is exactly the merge order.
fn append_document(
    dest: &mut Document,
    source: &Document,
    page_total: u32,
) -> Result<(), ToolError> {
    let mut cache: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
    for page_num in 1..=page_total {
        medpdf::copy_page_with_cache(dest, source, page_num, &mut cache).map_err(|e| {
            ToolError::Internal {
                detail: format!("复制第 {page_num} 页失败：{e}"),
            }
        })?;
    }
    Ok(())
}
