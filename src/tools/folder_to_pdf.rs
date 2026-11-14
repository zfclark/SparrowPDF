//! Tool: batch conversion — one PDF per input folder.
//!
//! * Images inside each folder are ordered by the number in their file name
//!   (`1.png`, `2.png`, …, `10.png` sorts numerically); names without any
//!   number keep the system listing order after the numbered ones.
//! * Page layout comes from a preset (`BATCH_TEMPLATES`): paper size, page
//!   orientation and one-or-two images per page. One image per page, or two
//!   images sharing one page (`BatchPageMode::TwoPerPage`, landscape = side by
//!   side, portrait = stacked), optionally with an explicit merge spec such as
//!   `2-3`. The chosen layout applies to *every* page of a document, including
//!   the pages that end up with a single image.
//! * Each folder becomes `<folder name>.pdf`, written into the folder itself
//!   by default or into the user-chosen output directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object};

use crate::engine::{pdf_io, validate};
use crate::model::{InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::image_embed::{self, add_page, ScanBudget, Sheet, PAGE_MARGIN};
use crate::tools::{
    check_cancel, ensure_dir, BatchPageMode, CancelFlag, Orientation, PageSize, Progress, Tool,
    ToolOptions, ToolRequest, SCAN_SIZE_LIMIT,
};

/// Outer margin between the page edge and the two image slots, in points.
const SLOT_MARGIN: f32 = 18.0;
/// Gap between the two image slots of a two-per-page layout, in points.
const SLOT_GAP: f32 = 18.0;

/// Page layout of one output document, decided once per folder.
///
/// Every page — single-image or two-image — is written from this one value.
/// The mode, not the page, owns the orientation: in two-per-page mode a page
/// that ends up with a single image (a trailing odd image, or one the merge
/// spec leaves alone) still uses the chosen orientation, paper size and margin,
/// so one PDF never mixes page sizes.
#[derive(Debug, Clone, Copy)]
struct PagePolicy {
    /// Orientation of pages holding one image; `None` follows each image.
    single_orientation: Option<Orientation>,
    /// Orientation of pages holding two images.
    pair_orientation: Orientation,
    /// Margin of a single-image page, in points.
    single_margin: f32,
    /// Paper size of every page.
    page_size: PageSize,
    /// Fixed slot size for card-size copies; `None` halves the page.
    slot_size: Option<(f32, f32)>,
    auto_rotate: bool,
    /// Storage budget for oversized scans (see `ScanQuality`). Carried here
    /// because the size guard re-renders a folder with a tighter budget.
    budget: Option<ScanBudget>,
}

impl PagePolicy {
    fn new(
        mode: BatchPageMode,
        force: Option<Orientation>,
        pair_orientation: Orientation,
        page_size: PageSize,
        slot_size: Option<(f32, f32)>,
        inset_single_pages: bool,
        auto_rotate: bool,
        budget: Option<ScanBudget>,
    ) -> Self {
        match mode {
            BatchPageMode::Single => Self {
                single_orientation: force,
                pair_orientation,
                single_margin: PAGE_MARGIN,
                page_size,
                slot_size,
                auto_rotate,
                budget,
            },
            // Two-per-page owns the page geometry for the whole document.
            // Presets may still ask for full-bleed single pages (入团志愿书
            // keeps its other pages "as usual").
            BatchPageMode::TwoPerPage => Self {
                single_orientation: Some(pair_orientation),
                pair_orientation,
                single_margin: if inset_single_pages {
                    SLOT_MARGIN
                } else {
                    PAGE_MARGIN
                },
                page_size,
                slot_size,
                auto_rotate,
                budget,
            },
        }
    }

    /// The same layout with a different scan budget (size-guard passes).
    fn with_budget(self, budget: Option<ScanBudget>) -> Self {
        Self { budget, ..self }
    }

    /// The orientation every page must share, when the layout fixes one.
    fn uniform_orientation(&self) -> Option<Orientation> {
        self.single_orientation
    }

    /// Where the images of one page are drawn.
    ///
    /// This is the only place that knows about margins, card slots and the
    /// two-slot geometry; `image_embed` just fits an image into what it is
    /// handed. Returning one [`Sheet`] per image is what lets page images be
    /// prepared off the main thread.
    fn sheets(&self, plan: &PagePlan) -> Vec<Sheet> {
        match plan {
            PagePlan::One(_) => vec![self.single_sheet()],
            PagePlan::Two(..) => self.pair_sheets().to_vec(),
        }
    }

    /// Sheet of a page holding a single image.
    fn single_sheet(&self) -> Sheet {
        let Some(orientation) = self.single_orientation else {
            // Follows the image: no forced direction, so no 90° turn either.
            return Sheet::Follow {
                page_size: self.page_size,
                margin: self.single_margin,
            };
        };
        let page = self.page_size.page(orientation);
        // A card template keeps its slot size on odd pages too, so one stray
        // 身份证 is not stretched over the whole sheet.
        let fit = self.slot_size.unwrap_or((
            page.0 - self.single_margin * 2.0,
            page.1 - self.single_margin * 2.0,
        ));
        Sheet::Fixed {
            page,
            fit,
            center: (page.0 / 2.0, page.1 / 2.0),
            landscape: orientation == Orientation::Landscape,
            auto_rotate: self.auto_rotate,
        }
    }

    /// Sheets of a two-image page.
    ///
    /// Landscape pages hold the slots side by side (portrait slots); portrait
    /// pages stack them vertically (landscape slots). `slot_size` switches to
    /// card-size copies (身份证): both slots keep their real dimensions and the
    /// pair is centred on the sheet, so the copy prints at 1:1 instead of being
    /// blown up to half a page.
    fn pair_sheets(&self) -> [Sheet; 2] {
        let (page_w, page_h) = self.page_size.page(self.pair_orientation);
        let (slot_w, slot_h, slots) = match self.slot_size {
            Some((w, h)) => {
                let slots = match self.pair_orientation {
                    Orientation::Landscape => {
                        let x = (page_w - (2.0 * w + SLOT_GAP)) / 2.0;
                        let y = (page_h - h) / 2.0;
                        [(x, y), (x + w + SLOT_GAP, y)]
                    }
                    Orientation::Portrait => {
                        let x = (page_w - w) / 2.0;
                        let y = (page_h - (2.0 * h + SLOT_GAP)) / 2.0;
                        [(x, y), (x, y + h + SLOT_GAP)]
                    }
                };
                (w, h, slots)
            }
            None => match self.pair_orientation {
                Orientation::Landscape => {
                    let (w, h) = (
                        (page_w - 2.0 * SLOT_MARGIN - SLOT_GAP) / 2.0,
                        page_h - 2.0 * SLOT_MARGIN,
                    );
                    (
                        w,
                        h,
                        [
                            (SLOT_MARGIN, SLOT_MARGIN),
                            (SLOT_MARGIN + w + SLOT_GAP, SLOT_MARGIN),
                        ],
                    )
                }
                Orientation::Portrait => {
                    let (w, h) = (
                        page_w - 2.0 * SLOT_MARGIN,
                        (page_h - 2.0 * SLOT_MARGIN - SLOT_GAP) / 2.0,
                    );
                    (
                        w,
                        h,
                        [
                            (SLOT_MARGIN, SLOT_MARGIN),
                            (SLOT_MARGIN, SLOT_MARGIN + h + SLOT_GAP),
                        ],
                    )
                }
            },
        };
        let landscape = slot_w > slot_h;
        slots.map(|(x, y)| Sheet::Fixed {
            page: (page_w, page_h),
            fit: (slot_w, slot_h),
            center: (x + slot_w / 2.0, y + slot_h / 2.0),
            landscape,
            auto_rotate: self.auto_rotate,
        })
    }
}

/// One folder's conversion job: what to read, how to lay it out, where it goes.
struct PlannedFolder<'a> {
    folder: &'a InputFile,
    images: Vec<PathBuf>,
    plans: Vec<PagePlan>,
    out_path: PathBuf,
}

pub struct FolderToPdf;

impl Tool for FolderToPdf {
    fn id(&self) -> ToolId {
        ToolId::FolderToPdf
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        if req.inputs.is_empty() {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请先添加至少一个文件夹".to_string(),
            });
        }
        for input in &req.inputs {
            if !input.path.is_dir() {
                return Err(ToolError::UnsupportedFormat {
                    path: Some(input.path.clone()),
                    detail: "不是文件夹或不存在".to_string(),
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
        let (
            template,
            mode,
            force,
            pair_orientation,
            page_size,
            slot_size,
            use_merge_spec,
            spec,
            auto_rotate,
            inset_single_pages,
            scan_quality,
        ) = match &req.options {
            ToolOptions::FolderToPdf {
                template,
                mode,
                force_orientation,
                pair_orientation,
                page_size,
                slot_size,
                use_merge_spec,
                merge_spec,
                auto_rotate,
                inset_single_pages,
                scan_quality,
            } => (
                *template,
                *mode,
                *force_orientation,
                *pair_orientation,
                *page_size,
                *slot_size,
                *use_merge_spec,
                merge_spec.clone(),
                *auto_rotate,
                *inset_single_pages,
                *scan_quality,
            ),
            // Every other tool reports a mismatched option set instead of
            // falling back to defaults: a silent fallback is precisely how a
            // layout setting ends up applied to only part of a document.
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        // Empty output path = default: each PDF lands inside its own folder.
        let out_root = if req.output.as_os_str().is_empty() {
            None
        } else {
            ensure_dir(&req.output)?;
            Some(req.output.clone())
        };

        let total = req.inputs.len();
        let mut outputs: Vec<PathBuf> = Vec::new();
        let mut used_names: HashSet<PathBuf> = HashSet::new();
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut total_images = 0usize;
        // Size telemetry for the summary: how big the largest artifact ended
        // up, and how many folders needed the 20 MB guard.
        let mut largest = 0u64;
        let mut compressed = 0usize;
        let mut oversized = 0usize;

        // Phase 1 — plan. Every output name is known before a single page is
        // rendered, so the caller can be asked about existing files up front
        // instead of after minutes of conversion.
        let mut planned: Vec<PlannedFolder<'_>> = Vec::new();
        for folder in &req.inputs {
            let images = collect_images(&folder.path);
            if images.is_empty() {
                skipped.push((
                    folder.display_name(),
                    "文件夹中没有可用的 JPG / PNG 图片".to_string(),
                ));
                continue;
            }

            let plans = match mode {
                BatchPageMode::Single => single_page_plans(images.len()),
                BatchPageMode::TwoPerPage => {
                    if use_merge_spec {
                        // Checked: only the user-listed images merge; an
                        // empty spec is a mistake, not a default.
                        if spec.trim().is_empty() {
                            return Err(ToolError::InvalidPageRange {
                                detail: "已勾选合并序号但未输入序号：请填写（如 2-3），或取消勾选以按顺序合并".to_string(),
                            });
                        }
                        // A preset's own spec is advisory (a folder with fewer
                        // images keeps those pages separate); a spec the user
                        // typed stays strict.
                        let strict = !template.owns_merge_spec(&spec);
                        parse_merge_spec(&spec, images.len(), strict)?
                    } else {
                        // Unchecked: pair everything sequentially.
                        sequential_pairs(images.len())
                    }
                }
            };

            let out_path = output_path(folder, out_root.as_deref(), &mut used_names);
            planned.push(PlannedFolder {
                folder,
                images,
                plans,
                out_path,
            });
        }

        let names: Vec<PathBuf> = planned.iter().map(|p| p.out_path.clone()).collect();
        let targets = pdf_io::resolve_outputs(&names, req.conflict)?;

        // Phase 2 — render and write.
        for (index, (job, out_path)) in planned.into_iter().zip(targets).enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!(
                    "处理文件夹 {}/{}：{}",
                    index + 1,
                    total,
                    job.folder.display_name()
                ),
            );

            // Built once per folder, then reused by every page.
            let policy = PagePolicy::new(
                mode,
                force,
                pair_orientation,
                page_size,
                slot_size,
                inset_single_pages,
                auto_rotate,
                scan_quality.budget(),
            );
            let (bytes, passes) = render_with_size_guard(
                &job,
                policy,
                cancel,
                progress,
                index,
                total,
                SCAN_SIZE_LIMIT,
            )?;

            // One unwritable / locked output must not discard the folders
            // that do succeed: record, log, continue — surfaced as
            // PartialSuccess at the end.
            let saved = pdf_io::write_file_atomic(&out_path, &bytes, req.conflict)
                .and_then(|_| validate::assert_page_count(&out_path, job.plans.len() as u32, None))
                // A fixed-orientation layout must hold for *every* page; a
                // mixed document means a page type lost the policy again.
                .and_then(|_| match policy.uniform_orientation() {
                    Some(orientation) => validate::assert_page_orientation(
                        &out_path,
                        orientation == Orientation::Landscape,
                    ),
                    None => Ok(()),
                });
            if let Err(err) = saved {
                crate::engine::logging::log(
                    "ERROR",
                    &format!("写入输出失败：{}（{err}）", out_path.display()),
                );
                skipped.push((job.folder.display_name(), format!("写入失败：{err}")));
                continue;
            }

            crate::engine::logging::log(
                "INFO",
                &format!(
                    "批量转换：{} → {:.1} MB（重压 {passes} 次）",
                    out_path.display(),
                    bytes.len() as f64 / (1024.0 * 1024.0)
                ),
            );
            if passes > 0 {
                compressed += 1;
            }
            if bytes.len() as u64 > SCAN_SIZE_LIMIT {
                oversized += 1;
            }
            largest = largest.max(bytes.len() as u64);
            outputs.push(out_path);
            total_images += job.images.len();
        }

        if outputs.is_empty() {
            let detail = skipped
                .iter()
                .map(|(name, reason)| format!("{name}: {reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有任何文件夹包含可用图片（{detail}）"),
            });
        }

        progress.report(1.0, "完成".to_string());

        let mut summary = format!(
            "已生成 {} 个 PDF（共 {total_images} 张图片，模板：{}，最大 {:.1} MB）",
            outputs.len(),
            template.label,
            largest as f64 / (1024.0 * 1024.0)
        );
        if compressed > 0 {
            summary.push_str(&format!("，{compressed} 个超出 20 MB 已重压"));
        }
        // The guard bottoms out at 96 DPI / quality 45; a scan that is still
        // huge after that must be reported, not silently shipped.
        if oversized > 0 {
            summary.push_str(&format!("，{oversized} 个压缩后仍超过 20 MB"));
        }
        if !skipped.is_empty() {
            let detail = skipped
                .iter()
                .map(|(name, reason)| format!("{name}: {reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::PartialSuccess {
                succeeded: outputs.len(),
                failed: skipped.len(),
                detail: format!("{summary}；跳过：{detail}"),
                outputs,
            });
        }

        Ok(ToolOutcome::success(summary, outputs))
    }
}

// ----------------------------------------------------------------- ordering

/// List a folder's JPG / PNG images in page order.
///
/// Files whose name contains a number sort by that number (`1`, `2`, …, `10`),
/// names without any number keep the system listing order after the numbered
/// ones (a stable sort with an `Equal` comparison preserves it).
pub fn collect_images(dir: &Path) -> Vec<PathBuf> {
    let mut items: Vec<(Option<u64>, String, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        items.push((first_number(&stem), name, path));
    }
    items.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then_with(|| a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    items.into_iter().map(|(_, _, path)| path).collect()
}

/// First run of digits in the stem as a number, ignoring leading zeros.
fn first_number(stem: &str) -> Option<u64> {
    let bytes = stem.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = stem[start..i].trim_start_matches('0');
            let digits = if digits.is_empty() {
                "0"
            } else {
                // Guard against absurdly long runs overflowing the parse.
                let take = digits.len().min(18);
                &digits[..take]
            };
            return digits.parse::<u64>().ok();
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------- page plans

/// Which images share a page (0-based indices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagePlan {
    One(usize),
    Two(usize, usize),
}

fn single_page_plans(count: usize) -> Vec<PagePlan> {
    (0..count).map(PagePlan::One).collect()
}

/// Pair every image sequentially: (1,2), (3,4), …; a trailing odd image
/// stays alone on the last page.
fn sequential_pairs(count: usize) -> Vec<PagePlan> {
    (0..count)
        .step_by(2)
        .map(|i| {
            if i + 1 < count {
                PagePlan::Two(i, i + 1)
            } else {
                PagePlan::One(i)
            }
        })
        .collect()
}

/// Output page number (1-based) of each image, for the thumbnail preview.
///
/// Lenient on purpose: an unusable merge spec falls back to sequential
/// pairing so the preview keeps working while the user types — the run
/// itself reports the exact error. `strict` must mirror the run's rule for
/// the spec at hand: a preset's own spec is advisory (out-of-range indices
/// are skipped), a spec the user typed is not.
pub fn preview_page_numbers(
    count: usize,
    two_per_page: bool,
    use_merge_spec: bool,
    merge_spec: &str,
    strict: bool,
) -> Vec<usize> {
    let plans = if !two_per_page {
        single_page_plans(count)
    } else if use_merge_spec && !merge_spec.trim().is_empty() {
        parse_merge_spec(merge_spec, count, strict).unwrap_or_else(|_| sequential_pairs(count))
    } else {
        sequential_pairs(count)
    };
    let mut numbers = vec![0usize; count];
    for (page_index, plan) in plans.iter().enumerate() {
        match *plan {
            PagePlan::One(i) => numbers[i] = page_index + 1,
            PagePlan::Two(a, b) => {
                numbers[a] = page_index + 1;
                numbers[b] = page_index + 1;
            }
        }
    }
    numbers
}

/// Parse the merge spec (`2-3`, `1-2,5-6`) into page plans.
///
/// * `A-B` puts images A and B on one page (order-agnostic, 1-based).
/// * `N` keeps image N alone.
/// * Images not mentioned in the spec are appended as single-image pages.
/// * Plans are ordered by the earliest image they contain, so output pages
///   follow the original image sequence (spec `2-3` yields pages
///   [1], [2+3], [4], …) instead of jumping to the spec pairs first.
///
/// With `strict == false` an index beyond `count` is skipped instead of
/// rejected: preset specs (入团志愿书's `2-3`) must still convert a folder
/// that holds fewer images. A malformed token is always an error — that is a
/// typing mistake, not a smaller folder.
fn parse_merge_spec(spec: &str, count: usize, strict: bool) -> Result<Vec<PagePlan>, ToolError> {
    let mut plans: Vec<PagePlan> = Vec::new();
    let mut used = vec![false; count];

    let mark_single = |idx: usize, used: &mut Vec<bool>| -> Result<(), ToolError> {
        if used[idx] {
            return Err(ToolError::InvalidPageRange {
                detail: format!("合并序号 {} 重复出现", idx + 1),
            });
        }
        used[idx] = true;
        Ok(())
    };

    for part in spec.split([',', '，', ' ', ';', '；']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once(['-', '~', '+']) {
            let Some(x) = parse_index(a, count, strict)? else {
                continue;
            };
            let Some(y) = parse_index(b, count, strict)? else {
                continue;
            };
            if x == y {
                mark_single(x, &mut used)?;
                plans.push(PagePlan::One(x));
            } else {
                let (lo, hi) = if x < y { (x, y) } else { (y, x) };
                if hi - lo > 1 {
                    return Err(ToolError::InvalidPageRange {
                        detail: format!(
                            "合并序号范围 {}-{} 包含 {} 张图片，一次只能合并两张",
                            lo + 1,
                            hi + 1,
                            hi - lo + 1
                        ),
                    });
                }
                mark_single(lo, &mut used)?;
                mark_single(hi, &mut used)?;
                plans.push(PagePlan::Two(lo, hi));
            }
        } else {
            let Some(x) = parse_index(part, count, strict)? else {
                continue;
            };
            mark_single(x, &mut used)?;
            plans.push(PagePlan::One(x));
        }
    }

    for (idx, was_used) in used.iter().enumerate() {
        if !was_used {
            plans.push(PagePlan::One(idx));
        }
    }
    // Keep output pages in the images' own order, merges inline.
    plans.sort_by_key(|plan| match plan {
        PagePlan::One(i) => *i,
        PagePlan::Two(a, _) => *a,
    });
    Ok(plans)
}

/// 1-based image number → 0-based index. `Ok(None)` means "beyond `count`"
/// while `strict` is off.
fn parse_index(raw: &str, count: usize, strict: bool) -> Result<Option<usize>, ToolError> {
    let trimmed = raw.trim();
    let n: usize = trimmed.parse().map_err(|_| ToolError::InvalidPageRange {
        detail: format!("「{trimmed}」不是合法的图片序号"),
    })?;
    if n == 0 {
        return Err(ToolError::InvalidPageRange {
            detail: "图片序号从 1 开始".to_string(),
        });
    }
    if n > count {
        if strict {
            return Err(ToolError::InvalidPageRange {
                detail: format!("合并序号 {n} 超出图片数量 {count}"),
            });
        }
        return Ok(None);
    }
    Ok(Some(n - 1))
}

// ---------------------------------------------------------------- rendering

/// Render one folder into the bytes to write, tightening the scan budget
/// while the artifact stays above [`SCAN_SIZE_LIMIT`].
///
/// The document is serialised in memory, so the exact byte length is known
/// before anything touches the disk — the file is written exactly once. A
/// tighter pass only re-renders; it costs seconds and only runs for documents
/// that would otherwise be uncomfortably large.
fn render_with_size_guard(
    job: &PlannedFolder<'_>,
    policy: PagePolicy,
    cancel: &CancelFlag,
    progress: &Progress,
    index: usize,
    total: usize,
    limit: u64,
) -> Result<(Vec<u8>, u32), ToolError> {
    let mut budget = policy.budget;
    let mut passes = 0u32;
    loop {
        let bytes = render_document(
            job,
            policy.with_budget(budget),
            cancel,
            progress,
            index,
            total,
        )?;
        if bytes.len() as u64 <= limit {
            return Ok((bytes, passes));
        }
        // `tightened` returns `None` at the ladder floor (`不压缩` included),
        // so an unfixable document is reported as-is instead of looping.
        let Some(tighter) = budget.and_then(ScanBudget::tightened) else {
            return Ok((bytes, passes));
        };
        budget = Some(tighter);
        passes += 1;
        progress.report(
            index as f32 / total as f32,
            format!(
                "{} 超过 {} MB，正在压缩（第 {passes} 次）",
                job.folder.display_name(),
                limit / (1024 * 1024)
            ),
        );
    }
}

/// Build one folder's PDF and serialise it.
fn render_document(
    job: &PlannedFolder<'_>,
    policy: PagePolicy,
    cancel: &CancelFlag,
    progress: &Progress,
    index: usize,
    total: usize,
) -> Result<Vec<u8>, ToolError> {
    // Images first (parallel, no PDF state), then the page objects (serial).
    let pages = prepare_pages(job, policy, cancel, progress, index, total)?;

    let mut doc = Document::with_version("1.7");
    let pages_id = build_document_skeleton(&mut doc);
    for (page_index, (plan, images)) in job.plans.iter().zip(pages).enumerate() {
        check_cancel(cancel)?;
        match plan {
            PagePlan::One(_) => {
                let Some(image) = images.into_iter().next() else {
                    return Err(ToolError::Internal {
                        detail: "单图页缺少图片".to_string(),
                    });
                };
                let page = image.page_box;
                let entries = vec![(format!("Im{}", page_index + 1), image)];
                add_page(&mut doc, pages_id, page, entries)?;
            }
            PagePlan::Two(..) => {
                // Both slots sit on the sheet the policy fixed for this page.
                let page = images.first().map(|image| image.page_box).ok_or_else(|| {
                    ToolError::Internal {
                        detail: "合并页缺少图片".to_string(),
                    }
                })?;
                let entries: Vec<_> = images
                    .into_iter()
                    .enumerate()
                    .map(|(slot, image)| (format!("Im{}-{}", page_index + 1, slot + 1), image))
                    .collect();
                add_page(&mut doc, pages_id, page, entries)?;
            }
        }
    }
    pdf_io::document_bytes(&mut doc)
}

/// How many images to prepare at once.
///
/// Decoding a 600 DPI RGBA page peaks around 200 MB, so the pool stays small
/// even on a 16-core machine: past four workers the batch is memory bound and
/// the extra threads only raise the peak.
const MAX_PREPARE_WORKERS: usize = 4;

fn worker_count(items: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    cores.min(MAX_PREPARE_WORKERS).min(items).max(1)
}

/// Prepare every image of a folder, several at a time, then regroup by page.
///
/// Preparation (decode → downsample → JPEG encode) is the whole cost of a scan
/// batch and shares no state, which is why it is spread over a few threads.
/// The results keep their page/slot order, so assembly stays a plain loop.
fn prepare_pages(
    job: &PlannedFolder<'_>,
    policy: PagePolicy,
    cancel: &CancelFlag,
    progress: &Progress,
    index: usize,
    total: usize,
) -> Result<Vec<Vec<image_embed::PreparedImage>>, ToolError> {
    struct Job<'a> {
        path: &'a Path,
        sheet: Sheet,
    }

    let mut jobs: Vec<Job<'_>> = Vec::with_capacity(job.plans.len());
    for plan in &job.plans {
        let images: &[usize] = match plan {
            PagePlan::One(index) => std::slice::from_ref(index),
            PagePlan::Two(left, right) => &[*left, *right],
        };
        for (image, sheet) in images.iter().zip(policy.sheets(plan)) {
            jobs.push(Job {
                path: &job.images[*image],
                sheet,
            });
        }
    }

    let mut done: Vec<Option<image_embed::PreparedImage>> = jobs.iter().map(|_| None).collect();
    // One report per finished image, so a long folder keeps the bar moving
    // even though the images themselves are prepared on worker threads.
    let prepared_count = std::sync::atomic::AtomicUsize::new(0);
    let folder_name = job.folder.display_name();
    let report = |progress: &Progress| {
        let finished = prepared_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let fraction = (index as f32 + finished as f32 / jobs.len() as f32) / total as f32;
        progress.report(
            fraction,
            format!("{folder_name}：压缩 {finished}/{} 张", jobs.len()),
        );
    };

    let workers = worker_count(jobs.len());
    if workers <= 1 {
        for (slot, job) in jobs.iter().enumerate() {
            check_cancel(cancel)?;
            done[slot] = Some(image_embed::prepare_image(
                job.path,
                job.sheet,
                policy.budget,
            )?);
            report(progress);
        }
    } else {
        let chunk = jobs.len().div_ceil(workers);
        std::thread::scope(|scope| -> Result<(), ToolError> {
            let handles: Vec<_> = jobs
                .chunks(chunk)
                .enumerate()
                .map(|(chunk_index, slice)| {
                    scope.spawn(
                        move || -> Result<Vec<(usize, image_embed::PreparedImage)>, ToolError> {
                            let mut out = Vec::with_capacity(slice.len());
                            for (offset, job) in slice.iter().enumerate() {
                                check_cancel(cancel)?;
                                out.push((
                                    chunk_index * chunk + offset,
                                    image_embed::prepare_image(job.path, job.sheet, policy.budget)?,
                                ));
                                report(progress);
                            }
                            Ok(out)
                        },
                    )
                })
                .collect();
            for handle in handles {
                let part = handle.join().map_err(|_| ToolError::Internal {
                    detail: "图片准备工作线程异常退出".to_string(),
                })??;
                for (slot, prepared) in part {
                    done[slot] = Some(prepared);
                }
            }
            Ok(())
        })?;
    }

    let mut done = done.into_iter();
    let mut pages: Vec<Vec<image_embed::PreparedImage>> = Vec::with_capacity(job.plans.len());
    for plan in &job.plans {
        let count = match plan {
            PagePlan::One(_) => 1,
            PagePlan::Two(..) => 2,
        };
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            let image = done.next().flatten().ok_or_else(|| ToolError::Internal {
                detail: "图片准备结果缺失".to_string(),
            })?;
            images.push(image);
        }
        pages.push(images);
    }
    Ok(pages)
}

fn build_document_skeleton(doc: &mut Document) -> lopdf::ObjectId {
    let pages_id = doc.new_object_id();
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

// ------------------------------------------------------------------ output

/// `<folder>.pdf` inside the folder itself, or inside the chosen output
/// directory; name collisions get a `-2`, `-3`, … suffix.
fn output_path(
    folder: &InputFile,
    out_root: Option<&Path>,
    used: &mut HashSet<PathBuf>,
) -> PathBuf {
    let name = folder
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let dir = out_root
        .map(|root| root.to_path_buf())
        .unwrap_or_else(|| folder.path.clone());

    let mut candidate = dir.join(format!("{name}.pdf"));
    let mut serial = 2;
    while !used.insert(candidate.clone()) {
        candidate = dir.join(format!("{name}-{serial}.pdf"));
        serial += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InputFile;
    use crate::tools::Progress;

    /// A4 portrait, one image per page — the layout a scan folder uses.
    fn scan_policy() -> PagePolicy {
        PagePolicy::new(
            BatchPageMode::Single,
            Some(Orientation::Portrait),
            Orientation::Portrait,
            PageSize::A4,
            None,
            false,
            true,
            Some(ScanBudget::new(200.0, 85)),
        )
    }

    /// A noisy raster at ~290 DPI: downsampling is the only way to shrink it,
    /// which is exactly what the size guard must do.
    fn noisy_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let v = ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 11) as u8;
            image::Rgb([v, v.wrapping_add(37), v.wrapping_mul(3)])
        });
        img.save_with_format(&path, image::ImageFormat::Png)
            .expect("write png fixture");
        path
    }

    #[test]
    fn size_guard_tightens_until_the_document_fits() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("sparrowpdf-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image_path = noisy_png(&dir, "1.png", 2400, 3400);

        let input = InputFile::new(dir.clone());
        let job = PlannedFolder {
            folder: &input,
            images: vec![image_path],
            plans: vec![PagePlan::One(0)],
            out_path: dir.join("out.pdf"),
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let progress = Progress::new(tx);
        let cancel: CancelFlag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Generous limit: one pass, no tightening.
        let (loose, passes) =
            render_with_size_guard(&job, scan_policy(), &cancel, &progress, 0, 1, u64::MAX)
                .expect("render");
        assert_eq!(passes, 0, "a document under the limit is written once");

        // An unreachable limit must still terminate, and every pass must make
        // the artifact smaller than the previous one.
        let (tight, passes) =
            render_with_size_guard(&job, scan_policy(), &cancel, &progress, 0, 1, 1)
                .expect("render");
        assert!(
            passes > 0,
            "the guard must tighten when the limit is missed"
        );
        assert!(
            tight.len() < loose.len(),
            "tightening must shrink the document: {} vs {}",
            tight.len(),
            loose.len()
        );

        // Nothing to tighten (原图直通): report the document as-is instead of
        // looping or failing.
        let passthrough = scan_policy().with_budget(None);
        let (raw, passes) =
            render_with_size_guard(&job, passthrough, &cancel, &progress, 0, 1, 1).expect("render");
        assert_eq!(passes, 0, "不压缩 must not attempt a re-render");
        assert!(raw.len() > tight.len());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
