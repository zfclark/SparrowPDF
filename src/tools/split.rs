//! Tool 3: split a PDF into several files.

use std::path::{Path, PathBuf};

use crate::engine::{pdf_io, validate};
use crate::model::{PageRangeMode, ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel, ensure_dir, CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct Split;

impl Tool for Split {
    fn id(&self) -> ToolId {
        ToolId::Split
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        let path = single_input(req)?;
        ensure_pdf(path)?;
        if let ToolOptions::Split {
            mode,
            spec,
            every_n,
        } = &req.options
        {
            match mode {
                PageRangeMode::Explicit => {
                    if spec.trim().is_empty() {
                        return Err(ToolError::InvalidPageRange {
                            detail: "请填写页码范围，例如 1-3,7".to_string(),
                        });
                    }
                }
                PageRangeMode::EveryN => {
                    if *every_n == 0 {
                        return Err(ToolError::InvalidPageRange {
                            detail: "每 N 页中的 N 必须大于 0".to_string(),
                        });
                    }
                }
                PageRangeMode::SinglePages => {}
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
        let path = single_input(req)?;
        let (mode, spec, every_n) = match &req.options {
            ToolOptions::Split {
                mode,
                spec,
                every_n,
            } => (*mode, spec.clone(), *every_n),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        let source = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&source);
        if total == 0 {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "文档没有页面".to_string(),
            });
        }

        let out_dir = split_output_dir(req);
        ensure_dir(&out_dir)?;
        let stem = file_stem(path);

        // Every mode reduces to a list of page groups, one output file per group.
        let groups: Vec<(String, Vec<u32>)> = match mode {
            PageRangeMode::SinglePages => (1..=total)
                .map(|p| (format!("{stem}_p{p:03}.pdf"), vec![p]))
                .collect(),
            PageRangeMode::Explicit => {
                vec![(
                    format!("{stem}_part1.pdf"),
                    pdf_io::parse_pages(&spec, total)?,
                )]
            }
            PageRangeMode::EveryN => (0..total)
                .step_by(every_n)
                .enumerate()
                .map(|(index, start)| {
                    let end = (start + every_n as u32).min(total);
                    (
                        format!("{stem}_part{:02}.pdf", index + 1),
                        (start + 1..=end).collect::<Vec<u32>>(),
                    )
                })
                .collect(),
        };

        // Every target name is known before the first file is written, so a
        // clash is reported once for the whole run instead of mid-way through.
        let planned: Vec<PathBuf> = groups.iter().map(|(name, _)| out_dir.join(name)).collect();
        let targets = pdf_io::resolve_outputs(&planned, req.conflict)?;

        let mut outputs: Vec<PathBuf> = Vec::with_capacity(groups.len());
        for (index, ((_, pages), out)) in groups.iter().zip(targets).enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / groups.len() as f32,
                format!("生成第 {}/{} 个文件", index + 1, groups.len()),
            );

            let mut dest = pdf_io::new_document();
            pdf_io::copy_pages(&mut dest, &source, pages)?;
            pdf_io::save_document(&mut dest, &out, req.conflict)?;
            validate::assert_page_count(&out, pages.len() as u32, None)?;
            outputs.push(out);
        }

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已拆分为 {} 个文件", outputs.len()),
            outputs,
        ))
    }
}

/// Where split output files go: the chosen directory, or the source's folder.
fn split_output_dir(req: &ToolRequest) -> PathBuf {
    let out = &req.output;
    let is_pdf_file = out
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if is_pdf_file {
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

pub(crate) fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string())
}

pub(crate) fn single_input(req: &ToolRequest) -> Result<&Path, ToolError> {
    match req.inputs.first() {
        Some(input) => Ok(&input.path),
        None => Err(ToolError::UnsupportedFormat {
            path: None,
            detail: "请先选择一个 PDF 文件".to_string(),
        }),
    }
}

/// Reject a document with no pages before a tool claims it processed it.
pub(crate) fn ensure_not_empty(total: u32, path: &Path) -> Result<(), ToolError> {
    if total == 0 {
        return Err(ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "文档没有页面".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn ensure_pdf(path: &Path) -> Result<(), ToolError> {
    if !path.is_file() {
        return Err(ToolError::ReadFailed {
            path: path.to_path_buf(),
            detail: "文件不存在".to_string(),
        });
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "pdf" {
        return Err(ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: format!("需要 PDF 文件，收到 .{ext}"),
        });
    }
    Ok(())
}
