//! Tool 11: replace pages with content from another PDF or a photo.
//!
//! Two files are involved and the panel keeps them apart:
//!
//! * **target** — the PDF whose pages get replaced; it is only read.
//! * **source** — a PDF page or a photo, whose content takes over the target
//!   page. A PDF page is imported as vector content (`contents` + `resources`
//!   deep-copied, resource names renamed by `medpdf`), so text stays text; a
//!   photo is embedded as an image XObject.
//!
//! The output is rebuilt page by page: kept pages are copied verbatim, replaced
//! pages come from the source. Output page numbers therefore match the target's,
//! which is what lets the panel preview the result before the run.

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel,
    image_embed::{self, fmt_num, Sheet},
    split::{ensure_not_empty, ensure_pdf, single_input},
    CancelFlag, Progress, ReplaceFit, ReplaceGeometry, ScanQuality, Tool, ToolOptions, ToolRequest,
};

/// Image extensions accepted in the source slot.
pub const IMAGE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// Whether `path` names a PDF (by extension — content is checked on open).
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Whether `path` names an image this tool can embed.
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            let ext = e.to_string_lossy().to_ascii_lowercase();
            IMAGE_EXTENSIONS.iter().any(|known| *known == ext)
        })
        .unwrap_or(false)
}

/// Which target pages are replaced, and with which source PDF page.
///
/// Kept as plain data: the tool and the preview both build it through
/// [`plan`], so the preview cannot show a different mapping than the run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplacementPlan {
    /// 1-based target pages, ascending.
    pub targets: Vec<u32>,
    /// Source PDF page for each target entry, in the same order. Empty when the
    /// source is an image (a photo is a single "page").
    pub source_pages: Vec<u32>,
}

/// One page of the rebuilt document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPage {
    /// Copied from the target document unchanged.
    Kept(u32),
    /// Taken from this page of the source PDF.
    FromPdf(u32),
    /// The image source, drawn on the target page's own sheet.
    FromImage,
}

/// Build the replacement plan from the two page specs.
///
/// `total_target` is the target's page count, `source_total` the source PDF's
/// (ignored for an image source).
pub fn plan(
    total_target: u32,
    source_total: Option<u32>,
    pages_spec: &str,
    source_spec: &str,
) -> Result<ReplacementPlan, ToolError> {
    let targets = pdf_io::parse_pages(pages_spec, total_target)?;
    let source_pages = match source_total {
        Some(total) => map_source_pages(targets.len(), total, source_spec)?,
        None => Vec::new(),
    };
    Ok(ReplacementPlan {
        targets,
        source_pages,
    })
}

/// Pair every replaced target page with a source page.
///
/// One source page (or an empty spec, which means page 1) is repeated for every
/// target page — the usual "stamp this page over those pages" case. Several
/// source pages must match the target count exactly: a silent cycle or truncate
/// would replace pages the user did not ask for.
pub fn map_source_pages(
    targets: usize,
    source_total: u32,
    spec: &str,
) -> Result<Vec<u32>, ToolError> {
    if targets == 0 {
        return Err(ToolError::InvalidPageRange {
            detail: "没有要替换的页面".to_string(),
        });
    }
    if source_total == 0 {
        return Err(ToolError::UnsupportedFormat {
            path: None,
            detail: "来源 PDF 没有页面".to_string(),
        });
    }
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Ok(vec![1; targets]);
    }
    let pages = pdf_io::parse_pages(trimmed, source_total)?;
    if pages.len() == 1 {
        return Ok(vec![pages[0]; targets]);
    }
    if pages.len() != targets {
        return Err(ToolError::InvalidPageRange {
            detail: format!(
                "已选 {targets} 个目标页，来源页填了 {} 个；两者数量需一致，或只填 1 页以重复使用",
                pages.len()
            ),
        });
    }
    Ok(pages)
}

/// Expand a plan into the output document's page sequence.
pub fn output_pages(total_target: u32, plan: &ReplacementPlan) -> Vec<OutputPage> {
    let mut slots: Vec<OutputPage> = (1..=total_target).map(OutputPage::Kept).collect();
    for (index, target) in plan.targets.iter().enumerate() {
        let Some(slot) = slots.get_mut(*target as usize - 1) else {
            continue;
        };
        *slot = match plan.source_pages.get(index) {
            Some(page) => OutputPage::FromPdf(*page),
            None => OutputPage::FromImage,
        };
    }
    slots
}

/// `2-4,7` for a sorted page list; used by the summary and the preview hint.
pub fn format_ranges(pages: &[u32]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < pages.len() {
        let start = pages[index];
        let mut end = start;
        while index + 1 < pages.len() && pages[index + 1] == end + 1 {
            index += 1;
            end = pages[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    parts.join(",")
}

pub struct ReplacePages;

impl Tool for ReplacePages {
    fn id(&self) -> ToolId {
        ToolId::ReplacePages
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        let target = single_input(req)?;
        ensure_pdf(target)?;

        let Some(source) = req.inputs.get(1) else {
            return Err(ToolError::UnsupportedFormat {
                path: None,
                detail: "请选择替换来源：一个 PDF 或一张 JPG / PNG 图片".to_string(),
            });
        };
        if !source.path.is_file() {
            return Err(ToolError::ReadFailed {
                path: source.path.clone(),
                detail: "替换来源不存在".to_string(),
            });
        }
        let pdf = is_pdf(&source.path);
        if !pdf && !is_image(&source.path) {
            let ext = source
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            return Err(ToolError::UnsupportedFormat {
                path: Some(source.path.clone()),
                detail: format!("替换来源仅支持 PDF / JPG / PNG，收到 .{ext}"),
            });
        }

        let ToolOptions::ReplacePages {
            pages,
            geometry,
            ..
        } = &req.options
        else {
            return Err(ToolError::Internal {
                detail: "选项类型不匹配".to_string(),
            });
        };
        if pages.trim().is_empty() {
            return Err(ToolError::InvalidPageRange {
                detail: "请填写要替换的页码，例如 3 或 2-4,7".to_string(),
            });
        }
        // A photo has no page box of its own: "follow the source size" would
        // have to invent one, so it is rejected instead of silently ignored.
        if !pdf && *geometry == ReplaceGeometry::FollowSource {
            return Err(ToolError::UnsupportedFormat {
                path: Some(source.path.clone()),
                detail: "图片来源没有页面尺寸，请选择「保持原页尺寸」".to_string(),
            });
        }
        Ok(())
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let target_path = single_input(req)?;
        let source_path = req
            .inputs
            .get(1)
            .map(|input| input.path.clone())
            .ok_or_else(|| ToolError::UnsupportedFormat {
                path: None,
                detail: "请选择替换来源：一个 PDF 或一张 JPG / PNG 图片".to_string(),
            })?;
        let (pages_spec, source_spec, fit, geometry) = match &req.options {
            ToolOptions::ReplacePages {
                pages,
                source_pages,
                fit,
                geometry,
            } => (pages.clone(), source_pages.clone(), *fit, *geometry),
            _ => {
                return Err(ToolError::Internal {
                    detail: "选项类型不匹配".to_string(),
                })
            }
        };

        // Plan the single output path before any rendering (AGENTS 3.2).
        let out = pdf_io::resolve_output(&req.output, req.conflict)?;
        // …and never let it land on one of the inputs: the default output sits
        // right next to the source, and picking the source itself under
        // 「覆盖」 would erase the only copy of it.
        pdf_io::ensure_output_is_not_input(&out, &[target_path, &source_path])?;

        let mut target_doc = pdf_io::open_for_edit(target_path, req.password.as_deref())?;
        let total = pdf_io::page_count(&target_doc);
        ensure_not_empty(total, target_path)?;
        let target_ids: BTreeMap<u32, ObjectId> = target_doc.get_pages();

        // `copy_page_*` copies a page dictionary verbatim, so a page that
        // inherits /Resources or /MediaBox from its /Pages node would arrive
        // blank and unsized. Materialise those values once, in memory only.
        for page_id in target_ids.values() {
            pdf_io::inherit_page_attributes(&mut target_doc, *page_id)?;
        }

        let image_source = !is_pdf(&source_path);
        let mut source_doc: Option<Document> = None;
        let plan = if image_source {
            plan(total, None, &pages_spec, &source_spec)?
        } else {
            let mut doc = pdf_io::open_document(&source_path, req.password.as_deref())?;
            let source_total = pdf_io::page_count(&doc);
            let plan = plan(total, Some(source_total), &pages_spec, &source_spec)?;
            let mut materialised: Vec<u32> = plan.source_pages.clone();
            materialised.sort_unstable();
            materialised.dedup();
            for page in materialised {
                if let Some(page_id) = doc.get_pages().get(&page).copied() {
                    pdf_io::inherit_page_attributes(&mut doc, page_id)?;
                }
            }
            source_doc = Some(doc);
            plan
        };

        let slots = output_pages(total, &plan);
        let mut out_doc = pdf_io::new_document();
        let pages_id = out_doc
            .catalog()
            .and_then(|catalog| catalog.get(b"Pages"))
            .and_then(Object::as_reference)
            .map_err(|e| ToolError::Internal {
                detail: format!("页面树损坏：{e}"),
            })?;

        let mut kept_cache: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
        let mut source_cache: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
        // A photo replacing several pages of the same size is embedded once and
        // drawn again; keyed by the integer point size so float noise cannot
        // produce a near-duplicate copy.
        let mut image_cache: BTreeMap<(i32, i32), (ObjectId, [f32; 6])> = BTreeMap::new();
        let mut expected_boxes: Vec<(u32, [f32; 4])> = Vec::new();

        for (index, slot) in slots.iter().enumerate() {
            check_cancel(cancel)?;
            let out_page = index as u32 + 1;
            progress.report(
                index as f32 / slots.len() as f32,
                format!("处理第 {out_page}/{} 页", slots.len()),
            );

            // The output keeps the target's page order, so the page being
            // replaced is the output page itself.
            let target_id = target_ids[&out_page];
            let target_size = pdf_io::displayed_page_size(&target_doc, target_id);

            match *slot {
                OutputPage::Kept(page) => {
                    pdf_io::copy_page(&mut out_doc, &target_doc, page, &mut kept_cache)?;
                }
                OutputPage::FromPdf(page) => {
                    let source_doc = source_doc.as_ref().ok_or_else(|| ToolError::Internal {
                        detail: "来源文档未打开".to_string(),
                    })?;
                    let new_id =
                        pdf_io::copy_page(&mut out_doc, source_doc, page, &mut source_cache)?;
                    // Stale annotations (links, form widgets) point at content
                    // that no longer exists on this page.
                    drop_annotations(&mut out_doc, new_id)?;
                    if geometry == ReplaceGeometry::KeepTarget {
                        let box4 = visible_box(&out_doc, new_id);
                        let rotation = pdf_io::page_rotation(&out_doc, new_id);
                        let prefix = content_prefix(box4, rotation, target_size, fit)?;
                        set_page_box(&mut out_doc, new_id, target_size)?;
                        wrap_content(&mut out_doc, new_id, prefix)?;
                        expected_boxes.push((out_page, [0.0, 0.0, target_size.0, target_size.1]));
                    }
                }
                OutputPage::FromImage => {
                    let key = (
                        target_size.0.round() as i32,
                        target_size.1.round() as i32,
                    );
                    let (xobject, matrix) = match image_cache.get(&key) {
                        Some(cached) => *cached,
                        None => {
                            let prepared = image_embed::prepare_image(
                                &source_path,
                                image_sheet(target_size, fit),
                                ScanQuality::Balanced.budget(),
                            )?;
                            let embedded = image_embed::embed_image(&mut out_doc, prepared)?;
                            image_cache.insert(key, embedded);
                            embedded
                        }
                    };
                    image_embed::add_drawn_page(
                        &mut out_doc, pages_id, target_size, "Im1", xobject, matrix,
                    )?;
                    expected_boxes.push((out_page, [0.0, 0.0, target_size.0, target_size.1]));
                }
            }
        }

        progress.report(0.9, "写入 PDF".to_string());
        check_cancel(cancel)?;
        pdf_io::save_document(&mut out_doc, &out, req.conflict)?;

        validate::assert_page_count(&out, total, None)?;
        validate::assert_page_boxes(&out, &expected_boxes)?;

        let replaced = plan.targets.len();
        let ranges = format_ranges(&plan.targets);
        let summary = if replaced == total as usize {
            format!("已替换全部 {replaced} 页（第 {ranges} 页）")
        } else {
            format!(
                "已替换 {replaced} 页（第 {ranges} 页），其余 {} 页保持原样",
                total as usize - replaced
            )
        };
        // Say it out loud when the paper changed, because that is visible in
        // the output and easy to mistake for a defect.
        let summary = if geometry == ReplaceGeometry::FollowSource {
            format!("{summary}；替换页尺寸跟随来源")
        } else {
            summary
        };

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(summary, vec![out]))
    }
}

/// Slot description for the image source: full page, centred.
fn image_sheet(page: (f32, f32), fit: ReplaceFit) -> Sheet {
    match fit {
        ReplaceFit::Contain => Sheet::Fixed {
            page,
            fit: page,
            center: (page.0 / 2.0, page.1 / 2.0),
            // The page direction is already fixed by the target; a turn here
            // would make the photo disagree with the sheet it replaces.
            landscape: page.0 > page.1,
            auto_rotate: false,
        },
        ReplaceFit::Cover => Sheet::Cover { page },
    }
}

/// Visible area of a page: `/CropBox` when it has one, else `/MediaBox`.
///
/// Fitting the full MediaBox would bring back the edges the source page
/// deliberately cropped away, because the output's own `/CropBox` is reset to
/// the whole sheet by [`set_page_box`].
fn visible_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    let media = pdf_io::page_media_box(doc, page_id);
    doc.get_object(page_id)
        .ok()
        .and_then(|object| object.as_dict().ok())
        .and_then(|dict| dict.get(b"CropBox").ok())
        .and_then(|object| box_array(doc, object))
        .map(normalize_box)
        .unwrap_or_else(|| normalize_box(media))
}

/// `[x0 y0 x1 y1]` of a page-box value, following one indirection.
fn box_array(doc: &Document, object: &Object) -> Option<[f32; 4]> {
    let resolved = match object {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    let items = resolved.as_array().ok()?;
    if items.len() != 4 {
        return None;
    }
    let mut corners = [0f32; 4];
    for (slot, item) in corners.iter_mut().zip(items) {
        *slot = item
            .as_float()
            .ok()
            .or_else(|| item.as_i64().ok().map(|value| value as f32))?;
    }
    Some(corners)
}

/// Both axes sorted low → high.
///
/// PDF lets a box be written as either pair of opposite corners; `[600 800 0 0]`
/// has the same size but would be read as "corner at (600, 800)" and push the
/// content off the sheet.
fn normalize_box(box4: [f32; 4]) -> [f32; 4] {
    [
        box4[0].min(box4[2]),
        box4[1].min(box4[3]),
        box4[0].max(box4[2]),
        box4[1].max(box4[3]),
    ]
}

/// Transform that maps a page's stored user space into its displayed space.
///
/// PDF matrices are `[a b c d e f]` with `x' = ax + cy + e`, `y' = bx + dy + f`.
/// `/Rotate` turns the page clockwise, so `/Rotate 90` sends the top-left corner
/// `(x0, y1)` to the top-right of the displayed `H × W` box.
fn display_matrix(box4: [f32; 4], rotation: u32) -> [f32; 6] {
    let [x0, y0, x1, y1] = box4;
    match rotation {
        90 => [0.0, -1.0, 1.0, 0.0, -y0, x1],
        180 => [-1.0, 0.0, 0.0, -1.0, x1, y1],
        270 => [0.0, 1.0, -1.0, 0.0, y1, -x0],
        _ => [1.0, 0.0, 0.0, 1.0, -x0, -y0],
    }
}

/// Content-stream prefix that turns the imported page into the target sheet.
///
/// Two `cm` operators: the first folds the source's own `/Rotate` into the
/// display frame, the second fits that frame into the target page. Writing them
/// as separate operators keeps the composition exact — a hand-multiplied matrix
/// is where sign and offset mistakes come from.
fn content_prefix(
    box4: [f32; 4],
    rotation: u32,
    target: (f32, f32),
    fit: ReplaceFit,
) -> Result<String, ToolError> {
    let box4 = normalize_box(box4);
    let (stored_w, stored_h) = ((box4[2] - box4[0]).abs(), (box4[3] - box4[1]).abs());
    let (display_w, display_h) = match rotation {
        90 | 270 => (stored_h, stored_w),
        _ => (stored_w, stored_h),
    };
    if display_w <= 0.0 || display_h <= 0.0 || target.0 <= 0.0 || target.1 <= 0.0 {
        return Err(ToolError::Internal {
            detail: "来源页或目标页尺寸为 0，无法计算替换位置".to_string(),
        });
    }
    let scale = fit.scale(target.0 / display_w, target.1 / display_h);
    let tx = (target.0 - display_w * scale) / 2.0;
    let ty = (target.1 - display_h * scale) / 2.0;
    let rotated = display_matrix(box4, rotation);
    Ok(format!(
        "q\n{} {} {} {} {} {} cm\n{} 0 0 {} {} {} cm\n",
        fmt_num(rotated[0]),
        fmt_num(rotated[1]),
        fmt_num(rotated[2]),
        fmt_num(rotated[3]),
        fmt_num(rotated[4]),
        fmt_num(rotated[5]),
        fmt_num(scale),
        fmt_num(scale),
        fmt_num(tx),
        fmt_num(ty),
    ))
}

/// Give a replaced page the target's page box and an upright orientation.
///
/// The page box is rebuilt from the *displayed* size, so a target page stored
/// sideways with `/Rotate 90` stays landscape on screen while its content is
/// written upright — the replacement follows the sheet the reader sees, not the
/// coordinates it happens to be stored in.
fn set_page_box(
    doc: &mut Document,
    page_id: ObjectId,
    size: (f32, f32),
) -> Result<(), ToolError> {
    let dict = doc
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("页面对象损坏：{e}"),
        })?;
    let box4 = vec![
        Object::Integer(0),
        Object::Integer(0),
        Object::Real(size.0),
        Object::Real(size.1),
    ];
    dict.set("MediaBox", box4.clone());
    dict.set("CropBox", box4);
    dict.set("Rotate", Object::Integer(0));
    // Optional boxes would still crop the page in some viewers.
    for key in [&b"BleedBox"[..], b"TrimBox", b"ArtBox"] {
        dict.remove(key);
    }
    Ok(())
}

/// Wrap a page's existing content in `q … Q` with a leading transform.
fn wrap_content(doc: &mut Document, page_id: ObjectId, prefix: String) -> Result<(), ToolError> {
    let existing = doc
        .get_object(page_id)
        .and_then(|object| object.as_dict())
        .and_then(|dict| dict.get(b"Contents"))
        .cloned();

    let mut streams: Vec<Object> = Vec::new();
    match existing {
        // `/Contents` may point at a *stream* or at an array of streams.
        // Pushing the array reference into the new array would nest an array
        // where a stream belongs — page count and boxes still validate, but
        // readers skip the page's only content.
        Ok(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(items)) => streams.extend(items.iter().cloned()),
            _ => streams.push(Object::Reference(id)),
        },
        Ok(Object::Array(items)) => streams.extend(items),
        Ok(other) => {
            return Err(ToolError::Internal {
                detail: format!("来源页 /Contents 不是流：{other:?}"),
            })
        }
        // A page without content stays blank, but still gets a balanced stream
        // so the page is not left half-transformed.
        Err(_) => {}
    }

    let pre = doc.add_object(Stream::new(Dictionary::new(), prefix.into_bytes()));
    let post = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));

    let mut array: Vec<Object> = Vec::with_capacity(streams.len() + 2);
    array.push(Object::Reference(pre));
    array.extend(streams);
    array.push(Object::Reference(post));

    doc.get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("页面对象损坏：{e}"),
        })?
        .set("Contents", Object::Array(array));
    Ok(())
}

/// Drop a replaced page's annotations.
///
/// Links, form widgets and comments belong to the content that was just
/// removed; keeping them alive would leave clickable areas over unrelated text.
fn drop_annotations(doc: &mut Document, page_id: ObjectId) -> Result<(), ToolError> {
    let dict = doc
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|e| ToolError::Internal {
            detail: format!("页面对象损坏：{e}"),
        })?;
    dict.remove(b"Annots");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_compacted() {
        assert_eq!(format_ranges(&[3]), "3");
        assert_eq!(format_ranges(&[2, 3, 4, 7]), "2-4,7");
        assert_eq!(format_ranges(&[1, 2, 5, 6, 9]), "1-2,5-6,9");
        assert_eq!(format_ranges(&[]), "");
    }

    /// 600×800 的来源放进 600×400 的页：宽度比例 1、高度比例 0.5。
    ///
    /// 「等比完整显示」必须留白（取较小的 0.5），「铺满页面」必须撑满（取较大
    /// 的 1）；按固定轴取比例会把留白版裁掉、把铺满版留白。
    #[test]
    fn fit_takes_the_smaller_ratio_to_contain_and_the_larger_to_cover() {
        let contain = content_prefix(
            [0.0, 0.0, 600.0, 800.0],
            0,
            (600.0, 400.0),
            ReplaceFit::Contain,
        )
        .expect("contain prefix");
        assert_eq!(contain, "q\n1 0 0 1 0 0 cm\n0.5 0 0 0.5 150 0 cm\n");
        let cover =
            content_prefix([0.0, 0.0, 600.0, 800.0], 0, (600.0, 400.0), ReplaceFit::Cover)
                .expect("cover prefix");
        assert_eq!(cover, "q\n1 0 0 1 0 0 cm\n1 0 0 1 0 -200 cm\n");
    }

    /// 角点可以按任一对对角写；`[600 800 0 0]` 与 `[0 0 600 800]` 是同一张纸。
    #[test]
    fn box_corners_are_normalized_before_the_transform() {
        let straight = content_prefix(
            [0.0, 0.0, 600.0, 800.0],
            0,
            (600.0, 800.0),
            ReplaceFit::Contain,
        )
        .expect("straight box");
        let reversed = content_prefix(
            [600.0, 800.0, 0.0, 0.0],
            0,
            (600.0, 800.0),
            ReplaceFit::Contain,
        )
        .expect("reversed box");
        assert_eq!(straight, reversed, "角点顺序不应改变变换");
    }

    #[test]
    fn one_source_page_is_repeated_for_every_target() {
        // Empty spec: page 1 everywhere.
        assert_eq!(map_source_pages(3, 5, "").unwrap(), vec![1, 1, 1]);
        // An explicit single page behaves the same way.
        assert_eq!(map_source_pages(2, 5, "4").unwrap(), vec![4, 4]);
    }

    #[test]
    fn several_source_pages_must_match_the_target_count() {
        assert_eq!(map_source_pages(2, 5, "2-3").unwrap(), vec![2, 3]);
        let err = map_source_pages(3, 5, "1-2").expect_err("count mismatch");
        assert!(matches!(err, ToolError::InvalidPageRange { .. }), "{err:?}");
        // Out-of-range source pages are rejected, not clamped.
        assert!(map_source_pages(1, 2, "9").is_err());
    }

    #[test]
    fn output_pages_keeps_target_numbering() {
        let plan = ReplacementPlan {
            targets: vec![2, 3],
            source_pages: vec![1, 1],
        };
        assert_eq!(
            output_pages(4, &plan),
            vec![
                OutputPage::Kept(1),
                OutputPage::FromPdf(1),
                OutputPage::FromPdf(1),
                OutputPage::Kept(4),
            ]
        );

        let image_plan = ReplacementPlan {
            targets: vec![1],
            source_pages: Vec::new(),
        };
        assert_eq!(
            output_pages(2, &image_plan),
            vec![OutputPage::FromImage, OutputPage::Kept(2)]
        );
    }

    #[test]
    fn display_matrix_maps_the_stored_box_onto_the_displayed_one() {
        let box4 = [0.0, 0.0, 600.0, 800.0];
        // Identity for an unrotated page.
        assert_eq!(display_matrix(box4, 0), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

        // 90° clockwise: the stored top-left corner ends up top-right of the
        // displayed 800 x 600 box, and the four corners cover that box exactly.
        let [a, b, c, d, e, f] = display_matrix(box4, 90);
        let map = |x: f32, y: f32| (a * x + c * y + e, b * x + d * y + f);
        assert_eq!(map(0.0, 800.0), (800.0, 600.0));
        assert_eq!(map(600.0, 800.0), (800.0, 0.0));
        assert_eq!(map(0.0, 0.0), (0.0, 600.0));
        assert_eq!(map(600.0, 0.0), (0.0, 0.0));

        // 270° is the opposite quarter turn.
        let [a, b, c, d, e, f] = display_matrix(box4, 270);
        let map = |x: f32, y: f32| (a * x + c * y + e, b * x + d * y + f);
        assert_eq!(map(0.0, 800.0), (0.0, 0.0));
        assert_eq!(map(600.0, 0.0), (800.0, 600.0));
    }
}
