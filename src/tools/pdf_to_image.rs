//! Tool 11: PDF to images, using the built-in rasterizer.
//!
//! Accepts one or more PDFs and writes every page as a PNG/JPEG into one output
//! folder. Names are `<pdf name>_p001.<ext>`, so pages of different documents
//! never collide.
//!
//! Every page is rasterized in-process by `engine::render` and encoded with the
//! `image` crate. A page that cannot be rendered is recorded and skipped
//! instead of producing a blank file, and one broken PDF does not abort the
//! rest of the batch.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lopdf::{Document, ObjectId};

use crate::engine::{pdf_io, pool, render, validate};
use crate::model::{ImageFormat, InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel, ensure_dir, CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct PdfToImage;

impl Tool for PdfToImage {
    fn id(&self) -> ToolId {
        ToolId::PdfToImage
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        if req.inputs.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请先添加至少一个 PDF 文件".to_string(),
            });
        }
        for input in &req.inputs {
            let is_pdf = input
                .path
                .extension()
                .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if !is_pdf {
                return Err(ToolError::UnsupportedFormat {
                    path: Some(input.path.clone()),
                    detail: "仅支持 PDF 文件".to_string(),
                });
            }
            if !input.path.is_file() {
                return Err(ToolError::ReadFailed {
                    path: input.path.clone(),
                    detail: "文件不存在".to_string(),
                });
            }
        }
        if let ToolOptions::PdfToImage { dpi, .. } = &req.options {
            if !(36..=600).contains(dpi) {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: format!("DPI 需在 36-600 之间，收到 {dpi}"),
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
        let (dpi, format) = match &req.options {
            ToolOptions::PdfToImage { dpi, format } => (*dpi, *format),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let inputs = dedupe(req.inputs.clone());
        if inputs.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "没有可转换的 PDF".to_string(),
            });
        }

        let out_dir = output_dir(req);
        ensure_dir(&out_dir)?;
        let password = req.password.as_deref();

        // Phase 1 — open every input for its page count and plan the image
        // names. Rendering at 300 DPI is slow, so a name clash has to be found
        // before the first page is rasterized, and an unreadable PDF must not
        // stop the ones after it.
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut planned: Vec<(PathBuf, String, Vec<u32>)> = Vec::new();
        let mut used: HashSet<PathBuf> = HashSet::new();
        let mut all_targets: Vec<PathBuf> = Vec::new();

        for input in &inputs {
            check_cancel(cancel)?;
            let name = input.display_name();
            progress.report(0.0, format!("读取 {name}"));

            let page_numbers: Vec<u32> = match pdf_io::open_for_edit(&input.path, password) {
                Ok(doc) => doc.get_pages().into_keys().collect(),
                Err(err) => {
                    skipped.push((name, err.to_string()));
                    continue;
                }
            };
            if page_numbers.is_empty() {
                skipped.push((name, "文档没有页面".to_string()));
                continue;
            }

            let stem = input
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "page".to_string());
            for page in &page_numbers {
                let base = format!("{stem}_p{page:03}");
                let mut candidate = out_dir.join(format!("{base}.{}", format.extension()));
                let mut serial = 2;
                // Two folders can hold the same file name; keep the second
                // set distinct instead of letting one overwrite the other.
                while !used.insert(candidate.clone()) {
                    candidate = out_dir.join(format!("{base}-{serial}.{}", format.extension()));
                    serial += 1;
                }
                all_targets.push(candidate);
            }
            planned.push((input.path.clone(), name, page_numbers));
        }

        if planned.is_empty() {
            let detail = skipped
                .iter()
                .map(|(name, reason)| format!("{name}：{reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有可转换的 PDF（{detail}）"),
            });
        }

        let resolved = pdf_io::resolve_outputs(&all_targets, req.conflict)?;

        // Phase 2 — render. Documents are reopened one at a time so a batch of
        // large PDFs does not keep every parse in memory; within one document
        // the pages are independent, so they rasterise on the shared pool.
        // The font set is shared by reference (its glyph cache is behind a
        // mutex) instead of being rebuilt per worker — one font collection is
        // tens of megabytes.
        let total: usize = all_targets.len();
        let mut outputs: Vec<PathBuf> = Vec::with_capacity(total);
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut cursor = 0usize;
        let mut finished = 0usize;

        let fonts = render::FontSet::system();
        if fonts.is_empty() {
            // Text will be omitted, but vector content still renders; say so once.
            progress.report(0.0, "未找到系统字体，文字内容将被跳过".to_string());
        }

        for (path, name, page_numbers) in &planned {
            let doc = match pdf_io::open_for_edit(path, password) {
                Ok(doc) => doc,
                Err(err) => {
                    failed.push((name.clone(), err.to_string()));
                    cursor += page_numbers.len();
                    continue;
                }
            };
            let pages = doc.get_pages();

            // Jobs are planned for the whole document first: every output name
            // was reserved in phase 1, and a missing page is recorded without
            // starting a worker for it.
            let mut jobs: Vec<(u32, ObjectId, PathBuf)> = Vec::with_capacity(page_numbers.len());
            for page_num in page_numbers {
                let out = resolved[cursor].clone();
                cursor += 1;
                match pages.get(page_num).copied() {
                    Some(page_id) => jobs.push((*page_num, page_id, out)),
                    None => {
                        failed.push((format!("{name} 第 {page_num} 页"), "页面不存在".to_string()))
                    }
                }
            }
            if jobs.is_empty() {
                continue;
            }

            let workers = pool::worker_count(jobs.len(), RENDER_WORKER_BYTES);
            pool::run(
                jobs.len(),
                workers,
                cancel,
                // One unrenderable page is skipped, not fatal: the rest of the
                // document is still worth exporting.
                pool::OnError::Continue,
                |index| {
                    let (page_num, page_id, out) = &jobs[index];
                    render_and_save(&doc, *page_id, *page_num, dpi, format, &fonts, out, req)
                },
                |index, result| {
                    let (page_num, _, out) = &jobs[index];
                    match result {
                        Ok(()) => outputs.push(out.clone()),
                        Err(err) => {
                            failed.push((format!("{name} 第 {page_num} 页"), err.to_string()))
                        }
                    }
                    finished += 1;
                    progress.report(
                        finished as f32 / total as f32,
                        format!("{name}：第 {page_num} 页"),
                    );
                    Ok(())
                },
            )?;
        }

        if outputs.is_empty() {
            let detail = failed
                .iter()
                .chain(skipped.iter())
                .map(|(name, reason)| format!("{name}：{reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有导出任何图片（{detail}）"),
            });
        }

        progress.report(1.0, "完成".to_string());
        let summary = format!(
            "已从 {} 个 PDF 导出 {} 张 {} 图片（{dpi} DPI）",
            planned.len(),
            outputs.len(),
            format.label()
        );

        let all_skipped = skipped.iter().chain(failed.iter()).count();
        if all_skipped > 0 {
            let detail = skipped
                .iter()
                .chain(failed.iter())
                .map(|(name, reason)| format!("{name}：{reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::PartialSuccess {
                succeeded: outputs.len(),
                failed: all_skipped,
                detail: format!("{summary}；跳过：{detail}"),
                outputs,
            });
        }

        Ok(ToolOutcome::success(summary, outputs))
    }
}

/// Peak memory one render worker needs: a 600 DPI A4 pixmap is ~140 MB, plus
/// the encoded byte buffer.
const RENDER_WORKER_BYTES: u64 = 192 * 1024 * 1024;

/// Render one page, encode it and write it through the shared save path.
fn render_and_save(
    doc: &Document,
    page_id: ObjectId,
    page_num: u32,
    dpi: u32,
    format: ImageFormat,
    fonts: &render::FontSet,
    out: &Path,
    req: &ToolRequest,
) -> Result<(), ToolError> {
    let pixmap = render::render_page(doc, page_id, dpi, fonts).map_err(|e| ToolError::Internal {
        detail: format!("第 {page_num} 页渲染失败：{e}"),
    })?;
    let (width, height) = (pixmap.width, pixmap.height);
    let bytes = match format {
        ImageFormat::Png => pixmap.to_png(),
        ImageFormat::Jpeg => pixmap.to_jpeg(),
    }
    .map_err(|e| ToolError::Internal {
        detail: format!("第 {page_num} 页编码失败：{e}"),
    })?;
    pdf_io::write_file(out, &bytes, req.conflict)?;
    validate::assert_image(out, width, height)
}

/// Drop repeated inputs, keeping the first occurrence.
fn dedupe(inputs: Vec<InputFile>) -> Vec<InputFile> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut unique = Vec::with_capacity(inputs.len());
    for input in inputs {
        let key = input
            .path
            .canonicalize()
            .unwrap_or_else(|_| input.path.clone());
        if seen.insert(key) {
            unique.push(input);
        }
    }
    unique
}

fn output_dir(req: &ToolRequest) -> PathBuf {
    let out = &req.output;
    let has_image_ext = out
        .extension()
        .map(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            e == "png" || e == "jpg" || e == "jpeg"
        })
        .unwrap_or(false);
    if has_image_ext {
        return out
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    if out.as_os_str().is_empty() {
        return req
            .inputs
            .first()
            .and_then(|i| i.path.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    out.clone()
}
