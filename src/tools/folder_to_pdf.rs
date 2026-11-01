//! Tool: batch conversion — one PDF per input folder.
//!
//! * Images inside each folder are ordered by the number in their file name
//!   (`1.png`, `2.png`, …, `10.png` sorts numerically); names without any
//!   number keep the system listing order after the numbered ones.
//! * Page layout: one image per page, or two images sharing one landscape A4
//!   page (`BatchPageMode::TwoPerPage`), optionally with an explicit merge
//!   spec such as `1-2,5-6`.
//! * Each folder becomes `<folder name>.pdf`, written into the folder itself
//!   by default or into the user-chosen output directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object};

use crate::engine::pdf_io;
use crate::model::{InputFile, ToolError, ToolId, ToolOutcome};
use crate::tools::image_embed::{
    self, add_image_xobject, attach_page, placement_matrix, read_exif_orientation,
    register_xobject, rotate_cw_code, write_content_stream, A4_HEIGHT, A4_WIDTH,
};
use crate::tools::{
    check_cancel, ensure_dir, BatchPageMode, CancelFlag, Orientation, Progress, Tool, ToolOptions,
    ToolRequest,
};

/// Outer margin between the page edge and the two image slots, in points.
const SLOT_MARGIN: f32 = 18.0;
/// Gap between the two image slots of a two-per-page layout, in points.
const SLOT_GAP: f32 = 18.0;

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
        let (mode, force, pair_orientation, use_merge_spec, spec, auto_rotate) = match &req.options {
            ToolOptions::FolderToPdf {
                mode,
                force_orientation,
                pair_orientation,
                use_merge_spec,
                merge_spec,
                auto_rotate,
            } => (
                *mode,
                *force_orientation,
                *pair_orientation,
                *use_merge_spec,
                merge_spec.clone(),
                *auto_rotate,
            ),
            _ => (
                BatchPageMode::Single,
                None,
                Orientation::Landscape,
                false,
                String::new(),
                true,
            ),
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

        for (index, folder) in req.inputs.iter().enumerate() {
            check_cancel(cancel)?;
            progress.report(
                index as f32 / total as f32,
                format!("处理文件夹 {}/{}：{}", index + 1, total, folder.display_name()),
            );

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
                                detail: "已勾选「指定合并序号」但未输入序号：请填写（如 1-2,5-6），或取消勾选以按顺序合并".to_string(),
                            });
                        }
                        parse_merge_spec(&spec, images.len())?
                    } else {
                        // Unchecked: original behaviour — pair everything.
                        sequential_pairs(images.len())
                    }
                }
            };

            let mut doc = Document::with_version("1.7");
            let pages_id = build_document_skeleton(&mut doc);
            for (page_index, plan) in plans.iter().enumerate() {
                check_cancel(cancel)?;
                render_page(
                    &mut doc,
                    pages_id,
                    page_index,
                    plan,
                    &images,
                    force,
                    pair_orientation,
                    auto_rotate,
                )?;
            }

            let out_path = output_path(folder, out_root.as_deref(), &mut used_names);
            // One unwritable / locked output must not discard the folders
            // that do succeed: record, log, continue — surfaced as
            // PartialSuccess at the end.
            let saved = pdf_io::save_document(&mut doc, &out_path).and_then(|_| {
                crate::engine::validate::assert_page_count(&out_path, plans.len() as u32, None)
            });
            if let Err(err) = saved {
                crate::engine::logging::log(
                    "ERROR",
                    &format!("写入输出失败：{}（{err}）", out_path.display()),
                );
                skipped.push((
                    folder.display_name(),
                    format!("写入失败：{err}"),
                ));
                continue;
            }

            outputs.push(out_path);
            total_images += images.len();
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

        let summary = format!(
            "已生成 {} 个 PDF（共 {total_images} 张图片，{} 模式）",
            outputs.len(),
            match mode {
                BatchPageMode::Single => "每页一张",
                BatchPageMode::TwoPerPage => "两图一页",
            }
        );
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
/// Lenient on purpose: an invalid merge spec falls back to sequential
/// pairing so the preview keeps working while the user types — the run
/// itself reports the exact error.
pub fn preview_page_numbers(
    count: usize,
    two_per_page: bool,
    use_merge_spec: bool,
    merge_spec: &str,
) -> Vec<usize> {
    let plans = if !two_per_page {
        single_page_plans(count)
    } else if use_merge_spec && !merge_spec.trim().is_empty() {
        parse_merge_spec(merge_spec, count).unwrap_or_else(|_| sequential_pairs(count))
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

/// Parse the merge spec (`1-2,5-6`) into page plans.
///
/// * `A-B` puts images A and B on one page (order-agnostic, 1-based).
/// * `N` keeps image N alone.
/// * Images not mentioned in the spec are appended as single-image pages.
/// * Plans are ordered by the earliest image they contain, so output pages
///   follow the original image sequence (spec `2-3` yields pages
///   [1], [2+3], [4], …) instead of jumping to the spec pairs first.
fn parse_merge_spec(spec: &str, count: usize) -> Result<Vec<PagePlan>, ToolError> {
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
            let x = parse_index(a, count)?;
            let y = parse_index(b, count)?;
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
            let x = parse_index(part, count)?;
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

fn parse_index(raw: &str, count: usize) -> Result<usize, ToolError> {
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
        return Err(ToolError::InvalidPageRange {
            detail: format!("合并序号 {n} 超出图片数量 {count}"),
        });
    }
    Ok(n - 1)
}

// ---------------------------------------------------------------- rendering

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

fn render_page(
    doc: &mut Document,
    pages_id: lopdf::ObjectId,
    page_index: usize,
    plan: &PagePlan,
    images: &[PathBuf],
    force: Option<Orientation>,
    pair_orientation: Orientation,
    auto_rotate: bool,
) -> Result<(), ToolError> {
    match plan {
        PagePlan::One(idx) => {
            image_embed::add_image_page(
                doc,
                pages_id,
                &images[*idx],
                force,
                auto_rotate,
                page_index,
            )?;
        }
        PagePlan::Two(left, right) => {
            add_two_image_page(
                doc,
                pages_id,
                page_index,
                (&images[*left], &images[*right]),
                pair_orientation,
                auto_rotate,
            )?;
        }
    }
    Ok(())
}

/// One A4 page with two image slots.
///
/// Landscape pages hold the slots side by side (portrait slots); portrait
/// pages stack them vertically (landscape slots). With `auto_rotate`, an
/// image whose orientation conflicts with its slot is turned 90° to fill it.
fn add_two_image_page(
    doc: &mut Document,
    pages_id: lopdf::ObjectId,
    page_index: usize,
    (left, right): (&Path, &Path),
    pair_orientation: Orientation,
    auto_rotate: bool,
) -> Result<(), ToolError> {
    let (page_w, page_h) = match pair_orientation {
        Orientation::Landscape => (A4_HEIGHT, A4_WIDTH),
        Orientation::Portrait => (A4_WIDTH, A4_HEIGHT),
    };
    let (slot_w, slot_h) = match pair_orientation {
        Orientation::Landscape => (
            (page_w - 2.0 * SLOT_MARGIN - SLOT_GAP) / 2.0,
            page_h - 2.0 * SLOT_MARGIN,
        ),
        Orientation::Portrait => (
            page_w - 2.0 * SLOT_MARGIN,
            (page_h - 2.0 * SLOT_MARGIN - SLOT_GAP) / 2.0,
        ),
    };
    let slot_is_landscape = slot_w > slot_h;

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), page_w.into(), page_h.into()],
        "Resources" => dictionary! {},
    });
    attach_page(doc, pages_id, page_id)?;

    let mut draws: Vec<(String, [f32; 6])> = Vec::new();
    for (slot, path) in [(0, left), (1, right)] {
        let image = medpdf_image::load_image(path).map_err(|e| ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: e.to_string(),
        })?;
        let is_jpeg = matches!(image, medpdf_image::ImageData::Jpeg { .. });
        let orientation = if is_jpeg { read_exif_orientation(path) } else { 1 };

        let (px_w, px_h) = (image.pixel_width() as f32, image.pixel_height() as f32);
        if px_w <= 0.0 || px_h <= 0.0 {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "图片尺寸为 0".to_string(),
            });
        }
        let (eff_w, eff_h) = if matches!(orientation, 5 | 6 | 7 | 8) {
            (px_h, px_w)
        } else {
            (px_w, px_h)
        };

        // The image is turned when its orientation conflicts with the slot.
        let image_is_landscape = eff_w > eff_h;
        let rotate = auto_rotate && image_is_landscape != slot_is_landscape;
        let (fit_w, fit_h) = if rotate {
            (eff_h, eff_w)
        } else {
            (eff_w, eff_h)
        };
        let scale = (slot_w / fit_w).min(slot_h / fit_h);
        let draw_w = fit_w * scale;
        let draw_h = fit_h * scale;

        let (slot_x, slot_y) = match pair_orientation {
            Orientation::Landscape => (
                SLOT_MARGIN + slot as f32 * (slot_w + SLOT_GAP),
                SLOT_MARGIN,
            ),
            Orientation::Portrait => (
                SLOT_MARGIN,
                SLOT_MARGIN + slot as f32 * (slot_h + SLOT_GAP),
            ),
        };
        let x0 = slot_x + (slot_w - draw_w) / 2.0;
        let y0 = slot_y + (slot_h - draw_h) / 2.0;

        let code = if rotate {
            rotate_cw_code(orientation)
        } else {
            orientation
        };
        let matrix = placement_matrix(code, x0, y0, draw_w, draw_h);

        let name = format!("Im{}-{}", page_index + 1, slot + 1);
        let xobj_id = add_image_xobject(doc, image)?;
        register_xobject(doc, page_id, &name, xobj_id)?;
        draws.push((name, matrix));
    }

    write_content_stream(doc, page_id, &draws)
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
