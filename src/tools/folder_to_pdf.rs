//! Tool: batch conversion — one PDF per input folder.
//!
//! * Images inside each folder are ordered by the number in their trailing
//!   segment (`1.png`, `2.png`, …, `10.png`, and `xxx_p001` sorts as 1);
//!   numbers earlier in the name are part of the base. Names without a
//!   trailing number keep the system listing order after the numbered ones.
//! * Page layout comes from a preset (`BATCH_TEMPLATES`): paper size, page
//!   orientation and one-or-two images per page. One image per page, or two
//!   images sharing one page (`BatchPageMode::TwoPerPage`, landscape = side by
//!   side, portrait = stacked), optionally with an explicit merge spec such as
//!   `2-3`. The chosen layout applies to *every* page of a document, including
//!   the pages that end up with a single image.
//! * Each folder becomes `<folder name>.pdf`, written into the folder itself
//!   by default or into the user-chosen output directory. With
//!   `BatchGrouping::ByName` the folder is split by base name instead:
//!   `xxx_1.png` + `xxx_2.png` become `xxx.pdf`, and every group is its own
//!   document for layouts, merge specs and the size guard.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object};

use crate::engine::{pdf_io, pool, validate};
use crate::model::{InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::image_embed::{self, add_page, ScanBudget, Sheet, PAGE_MARGIN};
use crate::tools::{
    check_cancel, ensure_dir, BatchGrouping, BatchPageMode, CancelFlag, Orientation, PageSize,
    Progress, Tool, ToolOptions, ToolRequest, SCAN_SIZE_LIMIT,
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

/// How one folder is laid out, exactly as the panel collects it.
///
/// Named fields instead of eight positional arguments: three of them are
/// booleans/options and swapping two would still compile.
#[derive(Debug, Clone, Copy)]
struct LayoutSpec {
    mode: BatchPageMode,
    /// Orientation of single-image pages; `None` follows each image.
    single_orientation: Option<Orientation>,
    /// Orientation of pages holding two images.
    pair_orientation: Orientation,
    page_size: PageSize,
    /// Fixed slot size for card-size copies; `None` halves the page.
    slot_size: Option<(f32, f32)>,
    /// Presets may ask for full-bleed single pages even in two-per-page mode.
    inset_single_pages: bool,
    auto_rotate: bool,
    /// Storage budget for oversized scans (see `ScanQuality`).
    budget: Option<ScanBudget>,
}

impl PagePolicy {
    fn new(spec: LayoutSpec) -> Self {
        let LayoutSpec {
            mode,
            single_orientation: force,
            pair_orientation,
            page_size,
            slot_size,
            inset_single_pages,
            auto_rotate,
            budget,
        } = spec;
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
    /// pages stack them vertically (landscape slots). Slot 0 is always the one
    /// read first: left in a landscape page, **upper** in a portrait page —
    /// see the comment on the stacked branch for why "upper" means "first".
    /// `slot_size` switches to card-size copies (身份证): both slots keep their
    /// real dimensions and the pair is centred on the sheet, so the copy prints
    /// at 1:1 instead of being blown up to half a page.
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
                    // A landscape slot on a portrait page means the image is
                    // drawn turned 90° clockwise, so the reader turns the sheet
                    // back counter-clockwise — and that maps the *upper* slot
                    // onto the left, the position read first. Stacking bottom-up
                    // here put the second image first on every merged page
                    // (入团志愿书 第 2、3 页 came out as 3, 2).
                    Orientation::Portrait => {
                        let x = (page_w - w) / 2.0;
                        let y = (page_h - (2.0 * h + SLOT_GAP)) / 2.0;
                        [(x, y + h + SLOT_GAP), (x, y)]
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
                // Same rule as the card-size branch above: slot 0 on top.
                Orientation::Portrait => {
                    let (w, h) = (
                        page_w - 2.0 * SLOT_MARGIN,
                        (page_h - 2.0 * SLOT_MARGIN - SLOT_GAP) / 2.0,
                    );
                    (
                        w,
                        h,
                        [
                            (SLOT_MARGIN, SLOT_MARGIN + h + SLOT_GAP),
                            (SLOT_MARGIN, SLOT_MARGIN),
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

/// One output document: what to read, how to lay it out, where it goes.
///
/// A folder produces one of these in `ByFolder` mode and one per base name in
/// `ByName` mode, so the render loop no longer assumes a folder per document.
struct PlannedOutput {
    /// Folder name, or `folder/group`, for progress and skip messages.
    label: String,
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
            grouping,
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
                grouping,
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
                *grouping,
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

        let mut outputs: Vec<PathBuf> = Vec::new();
        let mut used_names: HashSet<String> = HashSet::new();
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut total_images = 0usize;
        // Pages the run actually wrote: a merged folder has fewer pages than
        // images, and the summary used to show only the image count, which read
        // like a wrong page count.
        let mut total_pages = 0usize;
        // Size telemetry for the summary: how big the largest artifact ended
        // up, and how many folders needed the 20 MB guard.
        let mut largest = 0u64;
        let mut compressed = 0usize;
        let mut oversized = 0usize;

        // Phase 1 — plan. Every output name is known before a single page is
        // rendered, so the caller can be asked about existing files up front
        // instead of after minutes of conversion.
        let mut planned: Vec<PlannedOutput> = Vec::new();
        for folder in &req.inputs {
            let images = collect_images(&folder.path);
            if images.is_empty() {
                skipped.push((
                    folder.display_name(),
                    "文件夹中没有可用的 JPG / PNG 图片".to_string(),
                ));
                continue;
            }

            match grouping {
                BatchGrouping::ByFolder => {
                    // A preset's own spec is advisory (a folder with no image
                    // carrying one of those numbers keeps those pages
                    // separate); a spec the user typed stays strict.
                    let strict = !template.owns_merge_spec(&spec);
                    let plans = plan_pages(&images, mode, use_merge_spec, &spec, strict)?;
                    let out_path = output_path(folder, out_root.as_deref(), &mut used_names);
                    planned.push(PlannedOutput {
                        label: folder.display_name(),
                        images,
                        plans,
                        out_path,
                    });
                }
                BatchGrouping::ByName => {
                    // Validate the spec once so a typo still aborts before any
                    // rendering.
                    // A leftover spec from the two-per-page layout must not
                    // reject groups once the layout is one image per page.
                    let merge_active = mode == BatchPageMode::TwoPerPage && use_merge_spec;
                    if merge_active && !spec.trim().is_empty() {
                        parse_merge_spec(&spec, 1, None, false)?;
                    }
                    // A spec the user typed stays strict, but a group whose
                    // numbering cannot cover it is skipped on its own instead
                    // of discarding the whole run. A preset's own spec keeps
                    // its lenient rule.
                    let typed = merge_active && !template.owns_merge_spec(&spec);
                    if typed {
                        // Shape only (a malformed token, or a range wider than
                        // two images): that is a typo and aborts the batch.
                        spec_max_index(&spec)?;
                    }
                    let dir = out_root
                        .clone()
                        .unwrap_or_else(|| folder.path.clone());
                    let folder_name = folder.display_name();
                    for group in group_images(&images) {
                        let label = format!("{folder_name}/{}", group.name);
                        // Checked against this group's own numbering, so a
                        // group numbered 11–20 still satisfies "18-19".
                        let missing = if typed {
                            merge_spec_gaps(&group.images, &spec)
                        } else {
                            Vec::new()
                        };
                        if !missing.is_empty() {
                            skipped.push((
                                label,
                                format!(
                                    "合并序号 {} 在该组图片中不存在（共 {} 张）",
                                    format_numbers(&missing),
                                    group.images.len()
                                ),
                            ));
                            continue;
                        }
                        let plans = plan_pages(
                            &group.images,
                            mode,
                            use_merge_spec,
                            &spec,
                            typed,
                        )?;
                        let out_path = output_path_for(&dir, &group.name, &mut used_names);
                        planned.push(PlannedOutput {
                            label,
                            images: group.images,
                            plans,
                            out_path,
                        });
                    }
                }
            }
        }

        let total = planned.len().max(1);
        let names: Vec<PathBuf> = planned.iter().map(|p| p.out_path.clone()).collect();
        let targets = pdf_io::resolve_outputs(&names, req.conflict)?;

        // Phase 2 — render and write.
        for (index, (job, out_path)) in planned.into_iter().zip(targets).enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("处理 {}/{}：{}", index + 1, total, job.label),
            );

            // Built once per folder, then reused by every page.
            let policy = PagePolicy::new(LayoutSpec {
                mode,
                single_orientation: force,
                pair_orientation,
                page_size,
                slot_size,
                inset_single_pages,
                auto_rotate,
                budget: scan_quality.budget(),
            });
            // A folder that fails to render (a corrupt image, a decoder bug)
            // must not discard the folders that do succeed: record, log and
            // continue, exactly like a failed write. Only a cancellation from
            // the user stops the whole run.
            let (bytes, passes) = match render_with_size_guard(
                &job,
                policy,
                cancel,
                progress,
                index,
                total,
                SCAN_SIZE_LIMIT,
            ) {
                Ok(rendered) => rendered,
                Err(ToolError::Cancelled) => return Err(ToolError::Cancelled),
                Err(err) => {
                    crate::engine::logging::log(
                        "ERROR",
                        &format!("生成失败：{}（{err}）", out_path.display()),
                    );
                    skipped.push((job.label.clone(), format!("处理失败：{err}")));
                    continue;
                }
            };

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
                skipped.push((job.label.clone(), format!("写入失败：{err}")));
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
            total_pages += job.plans.len();
        }

        if outputs.is_empty() {
            let detail = skipped
                .iter()
                .map(|(name, reason)| format!("{name}: {reason}"))
                .collect::<Vec<_>>()
                .join("；");
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: format!("没有生成任何 PDF（{detail}）"),
            });
        }

        progress.report(1.0, "完成".to_string());

        let mut summary = format!(
            "已生成 {} 个 PDF（{total_images} 张图片、{total_pages} 页，模板：{}，最大 {:.1} MB）",
            outputs.len(),
            template.label,
            largest as f64 / (1024.0 * 1024.0)
        );
        if grouping == BatchGrouping::ByName {
            summary.push_str("，按文件名分组");
        }
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

// ----------------------------------------------------------------- planning

/// Plan one document's pages from its images.
///
/// `strict_spec` decides whether a number the folder does not contain is an
/// error (a spec the user typed for one folder) or is skipped (a preset's own
/// spec, and every group when one folder is split by name).
fn plan_pages(
    images: &[PathBuf],
    mode: BatchPageMode,
    use_merge_spec: bool,
    spec: &str,
    strict_spec: bool,
) -> Result<Vec<PagePlan>, ToolError> {
    let count = images.len();
    match mode {
        BatchPageMode::Single => Ok(single_page_plans(count)),
        BatchPageMode::TwoPerPage => {
            if use_merge_spec {
                // Checked: only the user-listed images merge; an empty spec is
                // a mistake, not a default.
                if spec.trim().is_empty() {
                    return Err(ToolError::InvalidPageRange {
                        detail: "已勾选合并序号但未输入序号：请填写（如 2-3），或取消勾选以按顺序合并"
                            .to_string(),
                    });
                }
                let numbers = own_numbers(images);
                parse_merge_spec(spec, count, numbers.as_deref(), strict_spec)
            } else {
                // Unchecked: pair everything sequentially.
                Ok(sequential_pairs(count))
            }
        }
    }
}

/// The number each image is referred to by, in `images` order (1-based).
///
/// Prefer the number the file name already carries (`form-2.jpg` → 2): that is
/// the number the user sees in Explorer, and the one a merge spec means. Only
/// when the folder has no usable numbering (no trailing number, or two files
/// sharing one) does the sorted position stand in for it.
pub fn image_numbers(images: &[PathBuf]) -> Vec<usize> {
    own_numbers(images).unwrap_or_else(|| (1..=images.len()).collect())
}

/// The images' own numbers, `None` when the folder cannot provide a unique one
/// for every image.
fn own_numbers(images: &[PathBuf]) -> Option<Vec<usize>> {
    if images.is_empty() {
        return None;
    }
    let mut numbers = Vec::with_capacity(images.len());
    for path in images {
        let stem = path.file_stem()?.to_string_lossy();
        numbers.push(usize::try_from(sequence_number(&stem)?).ok()?);
    }
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    sorted.dedup();
    // A duplicate number makes the mapping ambiguous: fall back to positions.
    (sorted.len() == numbers.len()).then_some(numbers)
}

/// Whether `numbers` is a gap-free run (`11,12,13`), which is how a folder that
/// continues an earlier scan is numbered.
fn is_contiguous(numbers: &[usize]) -> bool {
    numbers.windows(2).all(|pair| pair[1] == pair[0] + 1)
}

/// One output document produced from a folder that is split by file name.
pub struct ImageGroup {
    /// Base name, also the PDF stem.
    pub name: String,
    /// Images in page order.
    pub images: Vec<PathBuf>,
}

/// Split a folder's images by base name, preserving `collect_images` order.
///
/// `xxx_1.png` and `xxx_2.png` share the key `xxx`; `1.png` and `2.png` have no
/// usable base and stay separate, otherwise unrelated scans would be merged.
pub fn group_images(images: &[PathBuf]) -> Vec<ImageGroup> {
    let mut groups: Vec<(String, Vec<(u64, PathBuf)>)> = Vec::new();
    for path in images {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match split_sequence(&stem) {
            Some((base, number)) => match groups.iter_mut().find(|(key, _)| *key == base) {
                Some((_, entries)) => entries.push((number, path.clone())),
                None => groups.push((base, vec![(number, path.clone())])),
            },
            // No trailing sequence: this file is a document of its own. It
            // must not join `cover_1.png` just because both start with
            // `cover`, nor merge with a same-named `.jpg`.
            None => groups.push((stem, vec![(0, path.clone())])),
        }
    }

    groups
        .into_iter()
        .map(|(name, mut entries)| {
            // Sequence order within the group; equal numbers fall back to the
            // file name so the run is deterministic.
            entries.sort_by_key(|(number, path)| {
                (
                    *number,
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            });
            ImageGroup {
                name,
                images: entries.into_iter().map(|(_, path)| path).collect(),
            }
        })
        .collect()
}

/// One row of the by-name preview: what the run will produce for a group.
pub struct GroupPreview {
    pub name: String,
    /// Number of images in the group.
    pub images: usize,
    /// Page count, `None` when the group will be skipped.
    pub pages: Option<usize>,
    /// Why the group is skipped, in the same words the run reports.
    pub skip: Option<String>,
}

/// Plan the by-name preview without rendering anything.
///
/// Shares `group_images`, `merge_spec_gaps` and `preview_page_numbers` with the
/// run, so a group that would be skipped is shown as skipped instead of
/// advertising a PDF that never appears.
pub fn preview_groups(
    images: &[PathBuf],
    two_per_page: bool,
    use_merge_spec: bool,
    merge_spec: &str,
    typed_spec: bool,
) -> Vec<GroupPreview> {
    let merge_active = two_per_page && use_merge_spec && typed_spec;
    group_images(images)
        .into_iter()
        .map(|group| {
            // Same rule as the run: the numbers must exist in *this* group.
            let missing = if merge_active {
                merge_spec_gaps(&group.images, merge_spec)
            } else {
                Vec::new()
            };
            if !missing.is_empty() {
                return GroupPreview {
                    name: group.name,
                    images: group.images.len(),
                    pages: None,
                    skip: Some(format!(
                        "合并序号 {} 在该组图片中不存在（共 {} 张）",
                        format_numbers(&missing),
                        group.images.len()
                    )),
                };
            }
            // `preview_page_numbers` labels every image with its output page;
            // the largest label is the page count (merges repeat a page).
            let pages = preview_page_numbers(
                &group.images,
                two_per_page,
                use_merge_spec,
                merge_spec,
                typed_spec,
            )
            .into_iter()
            .max()
            .unwrap_or(0);
            GroupPreview {
                name: group.name,
                images: group.images.len(),
                pages: Some(pages),
                skip: None,
            }
        })
        .collect()
}

/// Separators allowed in front of a sequence token.
///
/// Covers what scanners and phones actually produce: ASCII `_ - . space`,
/// full-width `＿ － ． 　`, typographic dashes and the middle dot.
const SEQUENCE_SEPARATORS: [char; 11] = [
    '_', '-', ' ', '.', '·', '＿', '－', '—', '–', '．', '\u{3000}',
];

/// Split `<base><separator><token>` into the base and the sequence number.
///
/// Only the *last* segment counts: `xxx-1`, `yyy_001`, `zzz_1` and `aaa_p001`
/// are all sequence 1 with bases `xxx`, `yyy`, `zzz` and `aaa`. The token may
/// carry a short letter prefix (`p001`); digits earlier in the name are part of
/// the base (`2025年总结_1` → `2025年总结`). A wrapped token is accepted too:
/// `报告（2）`, `报告(2)`, `报告[2]`, `报告【2】`. `None` means there is no
/// trailing sequence, so the file is its own document.
fn split_sequence(stem: &str) -> Option<(String, u64)> {
    if let Some((base, token)) = unwrap_token(stem) {
        if let Some(parsed) = parse_sequence_token(base, token) {
            return Some(parsed);
        }
    }

    let index = stem.rfind(|ch| SEQUENCE_SEPARATORS.contains(&ch))?;
    // `rfind` yields a byte offset; full-width separators are three bytes, so
    // `+ 1` would slice inside the character and panic.
    let separator = stem[index..].chars().next()?;
    let base = trim_separators(&stem[..index]);
    let token = &stem[index + separator.len_utf8()..];
    parse_sequence_token(base, token)
}

/// Strip a trailing `()`, `（）`, `[]` or `【】` pair and return its inside.
fn unwrap_token(stem: &str) -> Option<(&str, &str)> {
    let last = stem.chars().next_back()?;
    let opener = match last {
        ')' => '(',
        '）' => '（',
        ']' => '[',
        '】' => '【',
        _ => return None,
    };
    let open = stem.rfind(opener)?;
    let start = open + opener.len_utf8();
    let end = stem.len() - last.len_utf8();
    Some((&stem[..open], &stem[start..end]))
}

/// `base` + trailing `optional letters + digits` → (base, number).
fn parse_sequence_token(base: &str, token: &str) -> Option<(String, u64)> {
    let token = token.trim();
    let digits = token
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let split = token.len() - digits;
    if !token[..split].chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    let number: u64 = token[split..].parse().ok()?;
    let base = trim_separators(base);
    if base.is_empty() {
        return None;
    }
    Some((base.to_string(), number))
}

fn trim_separators(text: &str) -> &str {
    text.trim_end_matches(|ch| SEQUENCE_SEPARATORS.contains(&ch))
        .trim()
}

// ----------------------------------------------------------------- ordering

/// List a folder's JPG / PNG images in page order.
///
/// Files sort by the sequence number in their **trailing** segment (`1`, `2`,
/// …, `10` for `p001`, `p2`, `p010`; also `1.png`, `2.png`); numbers elsewhere
/// in the name do not count. Files without a trailing number keep the system
/// listing order after the numbered ones (a stable `Equal` comparison
/// preserves it). This is the same rule the by-name grouping uses.
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
        items.push((sequence_number(&stem), name, path));
    }
    items.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then_with(|| a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    items.into_iter().map(|(_, _, path)| path).collect()
}

/// Sequence number used to order a folder's images.
///
/// Only the trailing segment counts (`xxx-1`, `yyy_001`, `aaa_p001`); a stem
/// that is nothing but digits is itself a sequence (`1`, `001`). Numbers
/// earlier in the name are part of the base, so `2025年总结_1` orders by 1
/// rather than by 2025. `None` keeps the file after the numbered ones in
/// system listing order.
fn sequence_number(stem: &str) -> Option<u64> {
    if let Some((_, number)) = split_sequence(stem) {
        return Some(number);
    }
    // Guard against absurdly long digit runs overflowing the parse.
    if !stem.is_empty() && stem.len() <= 18 && stem.bytes().all(|b| b.is_ascii_digit()) {
        return stem.parse::<u64>().ok();
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
/// the spec at hand: a preset's own spec is advisory (numbers the folder does
/// not contain are skipped), a spec the user typed is not.
pub fn preview_page_numbers(
    images: &[PathBuf],
    two_per_page: bool,
    use_merge_spec: bool,
    merge_spec: &str,
    strict: bool,
) -> Vec<usize> {
    let count = images.len();
    let plans = if !two_per_page {
        single_page_plans(count)
    } else if use_merge_spec && !merge_spec.trim().is_empty() {
        let numbers = own_numbers(images);
        parse_merge_spec(merge_spec, count, numbers.as_deref(), strict)
            .unwrap_or_else(|_| sequential_pairs(count))
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
/// Numbers are resolved by [`spec_index`]: the file name's own number first,
/// the sorted position only when the folder carries no numbering. With
/// `strict == false` a number the folder does not contain is skipped instead of
/// rejected: preset specs (入团志愿书's `2-3`) must still convert a folder that
/// has no such page. A malformed token is always an error — that is a typing
/// mistake, not a smaller folder.
fn parse_merge_spec(
    spec: &str,
    count: usize,
    numbers: Option<&[usize]>,
    strict: bool,
) -> Result<Vec<PagePlan>, ToolError> {
    let mut plans: Vec<PagePlan> = Vec::new();
    let mut used = vec![false; count];

    let mark_single = |idx: usize, used: &mut Vec<bool>| -> Result<(), ToolError> {
        if used[idx] {
            let number = numbers.map_or(idx + 1, |list| list[idx]);
            return Err(ToolError::InvalidPageRange {
                detail: format!("合并序号 {number} 重复出现"),
            });
        }
        used[idx] = true;
        Ok(())
    };

    // 1-based spec token → 0-based index (`None` = the folder has no such image).
    let resolve = move |raw: &str| -> Result<Option<usize>, ToolError> {
        spec_index(parse_spec_number(raw)?, count, numbers, strict)
    };
    // The number to show for an index: the image's own number when it has one.
    let shown = move |idx: usize| numbers.map_or(idx + 1, |list| list[idx]);

    for part in spec.split([',', '，', ' ', ';', '；']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once(['-', '~', '+']) {
            let Some(x) = resolve(a)? else {
                continue;
            };
            let Some(y) = resolve(b)? else {
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
                            shown(lo),
                            shown(hi),
                            hi - lo + 1
                        ),
                    });
                }
                mark_single(lo, &mut used)?;
                mark_single(hi, &mut used)?;
                plans.push(PagePlan::Two(lo, hi));
            }
        } else {
            let Some(x) = resolve(part)? else {
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

fn parse_spec_number(raw: &str) -> Result<usize, ToolError> {
    let trimmed = raw.trim();
    let n: usize = trimmed.parse().map_err(|_| ToolError::InvalidPageRange {
        detail: format!("「{trimmed}」不是合法的图片序号"),
    })?;
    if n == 0 {
        return Err(ToolError::InvalidPageRange {
            detail: "图片序号从 1 开始".to_string(),
        });
    }
    Ok(n)
}

/// Highest image number a spec refers to.
///
/// Used to decide whether a group is long enough before parsing it strictly:
/// a short group is skipped without turning the whole batch into an error.
fn spec_max_index(spec: &str) -> Result<usize, ToolError> {
    let mut max = 0usize;
    for part in spec.split([',', '，', ' ', ';', '；']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once(['-', '~', '+']) {
            let x = parse_spec_number(a)?;
            let y = parse_spec_number(b)?;
            let (lo, hi) = if x < y { (x, y) } else { (y, x) };
            if hi - lo > 1 {
                return Err(ToolError::InvalidPageRange {
                    detail: format!(
                        "合并序号范围 {lo}-{hi} 包含 {} 张图片，一次只能合并两张",
                        hi - lo + 1
                    ),
                });
            }
            max = max.max(hi);
        } else {
            max = max.max(parse_spec_number(part)?);
        }
    }
    Ok(max)
}

/// 1-based spec number → 0-based index into `images`.
///
/// The number is matched against the images' own numbers (`form-2.jpg` → 2)
/// whenever the folder has them, so a merge spec means the page the user sees
/// in Explorer — not whatever happens to sit at that position in the sorted
/// list. A folder that continues an earlier scan (`11–20`) keeps working: its
/// run is gap-free, so `2` still means the second image.
///
/// `Ok(None)` = "the folder has no such image" and `strict` is off (a preset's
/// advisory spec); a strict spec turns it into an error instead of silently
/// merging the wrong pages.
fn spec_index(
    number: usize,
    count: usize,
    numbers: Option<&[usize]>,
    strict: bool,
) -> Result<Option<usize>, ToolError> {
    let Some(list) = numbers else {
        // No usable numbering: the sorted position is the number.
        if number > count {
            if strict {
                return Err(ToolError::InvalidPageRange {
                    detail: format!("合并序号 {number} 超出图片数量 {count}"),
                });
            }
            return Ok(None);
        }
        return Ok(Some(number - 1));
    };

    if let Some(index) = list.iter().position(|candidate| *candidate == number) {
        return Ok(Some(index));
    }
    // A gap-free run that does not start at 1 (`11–20`): offset the number so
    // "the second image" keeps meaning the second image.
    if is_contiguous(list) {
        if let Some(target) = list[0].checked_add(number - 1) {
            if let Some(index) = list.iter().position(|candidate| *candidate == target) {
                return Ok(Some(index));
            }
        }
    }
    if strict {
        return Err(ToolError::InvalidPageRange {
            detail: format!("合并序号 {number} 在该文件夹的图片编号里不存在"),
        });
    }
    Ok(None)
}

/// Numbers a preset's merge spec asks for that the folder does not contain.
///
/// The preview uses this to say why a merge from a template did not happen
/// instead of silently showing one page per image.
pub fn merge_spec_gaps(images: &[PathBuf], spec: &str) -> Vec<usize> {
    let count = images.len();
    let numbers = own_numbers(images);
    let mut gaps: Vec<usize> = Vec::new();
    for part in spec.split([',', '，', ' ', ';', '；']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let tokens: [&str; 2] = match part.split_once(['-', '~', '+']) {
            Some((a, b)) => [a, b],
            None => [part, ""],
        };
        for token in tokens {
            if token.trim().is_empty() {
                continue;
            }
            let Ok(number) = parse_spec_number(token) else {
                continue;
            };
            if spec_index(number, count, numbers.as_deref(), false)
                .ok()
                .flatten()
                .is_none()
                && !gaps.contains(&number)
            {
                gaps.push(number);
            }
        }
    }
    gaps
}

/// Image numbers as the UI writes them: `3、4`.
pub fn format_numbers(numbers: &[usize]) -> String {
    numbers
        .iter()
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join("、")
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
    job: &PlannedOutput,
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
                job.label,
                limit / (1024 * 1024)
            ),
        );
    }
}

/// Build one folder's PDF and serialise it.
fn render_document(
    job: &PlannedOutput,
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

/// Peak memory one worker needs for a 600 DPI A4 page: the decoded RGBA buffer
/// (~140 MB), the RGB copy the resampler builds (~105 MB) and the output
/// raster (~35 MB), with room to spare. The pool turns this into a worker
/// count for the machine it runs on.
const PREPARE_WORKER_BYTES: u64 = 400 * 1024 * 1024;

/// One image waiting to be prepared: where it belongs on the page, where to
/// read it, and how it is laid out.
struct PrepareJob<'a> {
    page: usize,
    slot: usize,
    path: &'a Path,
    sheet: Sheet,
}

/// Prepare every image of a folder concurrently, then regroup by page.
///
/// Only `prepare_image` (decode → downsample → JPEG encode) runs in parallel:
/// it touches no `Document` and shares nothing but the job list. PDF objects,
/// conflict checks and the file write stay on the calling thread, because they
/// mutate the document and must keep page order.
///
/// The pool reports each finished image on this thread, so progress stays
/// monotonic and each result lands in its slot immediately; prepared payloads
/// in flight are bounded by the pool's queue, never by the page count.
fn prepare_pages(
    job: &PlannedOutput,
    policy: PagePolicy,
    cancel: &CancelFlag,
    progress: &Progress,
    folder_index: usize,
    folders: usize,
) -> Result<Vec<Vec<image_embed::PreparedImage>>, ToolError> {
    let mut jobs: Vec<PrepareJob<'_>> = Vec::with_capacity(job.plans.len());
    for (page, plan) in job.plans.iter().enumerate() {
        let images: &[usize] = match plan {
            PagePlan::One(index) => std::slice::from_ref(index),
            PagePlan::Two(left, right) => &[*left, *right],
        };
        for (slot, (image, sheet)) in images.iter().zip(policy.sheets(plan)).enumerate() {
            jobs.push(PrepareJob {
                page,
                slot,
                path: &job.images[*image],
                sheet,
            });
        }
    }
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let total_images = jobs.len();
    let workers = pool::worker_count(total_images, PREPARE_WORKER_BYTES);
    let folder_name = job.label.clone();
    let mut prepared: Vec<Option<image_embed::PreparedImage>> = jobs.iter().map(|_| None).collect();
    let mut finished = 0usize;

    pool::run(
        total_images,
        workers,
        cancel,
        pool::OnError::Stop,
        |index: usize| {
            image_embed::prepare_image(jobs[index].path, jobs[index].sheet, policy.budget)
        },
        |index, result| match result {
            Ok(image) => {
                prepared[index] = Some(image);
                finished += 1;
                let fraction =
                    (folder_index as f32 + finished as f32 / total_images as f32) / folders as f32;
                progress.report(
                    fraction,
                    format!("{folder_name}：压缩 {finished}/{total_images} 张"),
                );
                Ok(())
            }
            // Report it to the pool: the run stops and this error becomes the
            // folder's failure reason.
            Err(err) => Err(err),
        },
    )?;

    // Jobs are pushed page by page, slot by slot, so appending in job order
    // keeps each page's draw order.
    let mut pages: Vec<Vec<image_embed::PreparedImage>> =
        job.plans.iter().map(|_| Vec::new()).collect();
    for (index, prepare_job) in jobs.iter().enumerate() {
        let image = prepared[index].take().ok_or_else(|| ToolError::Internal {
            detail: "图片准备结果缺失".to_string(),
        })?;
        // Appending in job order must land on the job's own slot; a mismatch
        // would silently swap the two images of a merged page.
        debug_assert_eq!(pages[prepare_job.page].len(), prepare_job.slot);
        pages[prepare_job.page].push(image);
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
    used: &mut HashSet<String>,
) -> PathBuf {
    let name = folder
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let dir = out_root
        .map(|root| root.to_path_buf())
        .unwrap_or_else(|| folder.path.clone());
    output_path_for(&dir, &name, used)
}

/// `<dir>/<name>.pdf`, with `-2`, `-3` … when the name is already taken.
///
/// The key folds case on Windows and macOS: `A.pdf` and `a.pdf` are the same
/// file there, so planning both as separate outputs would silently overwrite
/// one with the other under `Overwrite`/`Unique`.
fn output_path_for(dir: &Path, name: &str, used: &mut HashSet<String>) -> PathBuf {
    let mut candidate = dir.join(format!("{name}.pdf"));
    let mut serial = 2;
    while !used.insert(output_key(&candidate)) {
        candidate = dir.join(format!("{name}-{serial}.pdf"));
        serial += 1;
    }
    candidate
}

fn output_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InputFile;
    use crate::tools::Progress;

    /// A4 portrait, one image per page — the layout a scan folder uses.
    fn scan_policy() -> PagePolicy {
        PagePolicy::new(LayoutSpec {
            mode: BatchPageMode::Single,
            single_orientation: Some(Orientation::Portrait),
            pair_orientation: Orientation::Portrait,
            page_size: PageSize::A4,
            slot_size: None,
            inset_single_pages: false,
            auto_rotate: true,
            budget: Some(ScanBudget::new(200.0, 85)),
        })
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

    /// Slot centres of a two-image page.
    fn pair_policy(orientation: Orientation, slot_size: Option<(f32, f32)>) -> PagePolicy {
        PagePolicy::new(LayoutSpec {
            mode: BatchPageMode::TwoPerPage,
            single_orientation: Some(orientation),
            pair_orientation: orientation,
            page_size: PageSize::A4,
            slot_size,
            inset_single_pages: false,
            auto_rotate: true,
            budget: None,
        })
    }

    fn center(sheet: &Sheet) -> (f32, f32) {
        match sheet {
            Sheet::Fixed { center, .. } => *center,
            Sheet::Cover { page } => (page.0 / 2.0, page.1 / 2.0),
            Sheet::Follow { .. } => panic!("a two-image page always has fixed slots"),
        }
    }

    /// Portrait pair pages stack the slots upper-first.
    ///
    /// The slots are landscape, so a portrait image is drawn turned 90°
    /// clockwise and the reader turns the sheet back counter-clockwise — that
    /// puts the upper slot on the left, the position read first. Stacking
    /// bottom-up is what swapped the 入团志愿书's pages 2 and 3.
    #[test]
    fn portrait_pair_puts_the_first_image_in_the_upper_slot() {
        for slot_size in [None, Some((242.65, 153.07))] {
            let [first, second] = pair_policy(Orientation::Portrait, slot_size).pair_sheets();
            let (x0, y0) = center(&first);
            let (x1, y1) = center(&second);
            assert!((x0 - x1).abs() < 0.01, "slots share one column: {x0} vs {x1}");
            assert!(y0 > y1, "slot 0 must be the upper one: {y0} vs {y1}");
        }
    }

    /// Landscape pair pages keep the usual left-to-right order.
    #[test]
    fn landscape_pair_puts_the_first_image_on_the_left() {
        for slot_size in [None, Some((242.65, 153.07))] {
            let [first, second] = pair_policy(Orientation::Landscape, slot_size).pair_sheets();
            let (x0, y0) = center(&first);
            let (x1, y1) = center(&second);
            assert!((y0 - y1).abs() < 0.01, "slots share one row: {y0} vs {y1}");
            assert!(x0 < x1, "slot 0 must be the left one: {x0} vs {x1}");
        }
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
        let job = PlannedOutput {
            label: input.display_name(),
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
