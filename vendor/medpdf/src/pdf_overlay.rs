//! Page overlay — merging content from one PDF page onto another with resource renaming.

use crate::error::{MedpdfError, Result};
use crate::pdf_helpers::{self, KEY_CONTENTS, KEY_KIDS, KEY_PAGE, KEY_PAGES, KEY_RESOURCES, KEY_TYPE};
use log::{debug, trace, warn};
use lopdf::content::Operation;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, HashMap, HashSet};

fn add_resource_keys(keys: &mut HashSet<Vec<u8>>, dict_resources: &Dictionary) {
    for (_, value) in dict_resources.iter() {
        if let Object::Dictionary(dict) = value {
            for (key, _) in dict.iter() {
                keys.insert(key.clone());
            }
        }
    }
}

/// Collects resource key names from a document's page tree, starting at `start`.
///
/// Recurses into child `/Pages` nodes and individual `/Page` nodes so that
/// per-page `/Resources` dictionaries are also collected, preventing
/// resource-name collisions during overlay.
fn accumulate_dictionary_keys(
    keys: &mut HashSet<Vec<u8>>,
    doc: &Document,
    start: ObjectId,
) -> Result<()> {
    let o = doc.get_object(start)?;
    let dict = match o {
        Object::Dictionary(d) => d,
        _ => return Ok(()),
    };

    let is_page_node = match dict.get(KEY_TYPE) {
        Ok(Object::Name(v)) => v == KEY_PAGES || v == KEY_PAGE,
        _ => false,
    };
    if !is_page_node {
        return Ok(());
    }

    // Collect resource keys from this node's /Resources
    match dict.get(KEY_RESOURCES) {
        Ok(Object::Dictionary(dict_resources)) => {
            add_resource_keys(keys, dict_resources);
        }
        Ok(Object::Reference(id_resources)) => {
            if let Ok(dict_resources) = doc.get_dictionary(*id_resources) {
                add_resource_keys(keys, dict_resources);
            }
        }
        _ => {}
    }

    // Recurse into /Kids for /Pages nodes
    if let Ok(Object::Array(kids)) = dict.get(KEY_KIDS) {
        let child_ids: Vec<ObjectId> = kids
            .iter()
            .filter_map(|obj| obj.as_reference().ok())
            .collect();
        for child_id in child_ids {
            accumulate_dictionary_keys(keys, doc, child_id)?;
        }
    }

    Ok(())
}

fn find_unique_name(
    keys_used: &HashSet<Vec<u8>>,
    key_old: &[u8],
    suffix: &[u8],
) -> Result<Vec<u8>> {
    let mut buffer = Vec::<u8>::with_capacity(key_old.len() + suffix.len() + 5);
    buffer.extend_from_slice(key_old);
    buffer.extend_from_slice(suffix);
    let start_len = buffer.len();
    let mut itoa_buf = itoa::Buffer::new();
    for i in 0..10_000 {
        if i > 0 {
            buffer.truncate(start_len);
            buffer.extend_from_slice(itoa_buf.format(i).as_bytes());
        }
        if !keys_used.contains(&buffer) {
            return Ok(buffer);
        }
    }
    Err(MedpdfError::new("No new unique key could be generated"))
}

fn rename_resources_in_dict(
    key_mapping: &mut HashMap<Vec<u8>, Vec<u8>>,
    keys_used: &mut HashSet<Vec<u8>>,
    dest_doc: &mut Document,
    resources_dict_id_new: ObjectId,
) -> Result<()> {
    let dict = dest_doc.get_dictionary_mut(resources_dict_id_new)?;
    let new_key_suffix = vec![b'_', b'o'];
    for (_, value) in dict.iter_mut() {
        // The unused "key" here is /Font, /XObject, etc.  We don't need to know what key it is
        // When value is a dictionary, it contains key->value pairs for resources.  We can ignore non-dictionary values
        if let Object::Dictionary(dict) = value {
            let list_of_keys = dict
                .iter()
                .map(|(k, _v)| k.clone())
                .collect::<Vec<Vec<u8>>>();
            for key in list_of_keys {
                // If we've already mapped this key, skip regenerating new key as we need to
                // preserve the mapping from old keys (that may be shadowing across multiple
                // dictionaries from the source overlay document).
                if key_mapping.contains_key(&key) {
                    continue;
                }

                // These key/value pairs are a resource_name/resource_value pair.  We need to rename the name.
                let key_new = find_unique_name(keys_used, &key, &new_key_suffix)?;
                key_mapping.insert(key.clone(), key_new.clone());
                if let Some(v) = dict.remove(&key) {
                    dict.set(key_new, v);
                }

                // NO!: keys_used.insert(key);  (See note above of about preserving overlapping keys from source document.)
            }
        }
    }
    Ok(())
}

fn modify_content_stream(
    dest_doc: &mut Document,
    contents_arr: &[Object],
    key_mapping: Option<&HashMap<Vec<u8>, Vec<u8>>>,
) -> Result<()> {
    let len = contents_arr.len();
    let mut cumulative_q_balance = 0_isize;

    for (idx, content_ref_obj) in contents_arr.iter().enumerate() {
        let content_stream = dest_doc
            .get_object_mut(content_ref_obj.as_reference()?)?
            .as_stream_mut()?;
        if content_stream.is_compressed() {
            content_stream.decompress()?;
        }
        let mut content = content_stream.decode_content()?;
        for operation in content.operations.iter_mut() {
            match &operation.operator[..] {
                "q" => cumulative_q_balance += 1,
                "Q" => cumulative_q_balance -= 1,
                _ => {}
            }
            if let Some(key_mapping) = key_mapping {
                for operand in operation.operands.iter_mut() {
                    // Process only operands that are names...
                    if let Ok(name) = operand.as_name() {
                        // ...that also have a mapping to a new name
                        if let Some(name_new) = key_mapping.get(name) {
                            *operand = Object::Name(name_new.clone());
                        }
                    }
                }
            }
        }
        // Wrap the entire sequence of content streams in a single q/Q pair,
        // rather than wrapping each stream individually. Multiple content streams
        // for one page are concatenated — graphics state must carry across them.
        if idx == 0 {
            content.operations.insert(0, Operation::new("q", vec![]));
        }
        if idx == len - 1 {
            content.operations.push(Operation::new("Q", vec![]));
            // Balance any unmatched q operators across all streams
            if cumulative_q_balance < 0 {
                warn!("Content streams have {} more Q than q operators (negative balance)", -cumulative_q_balance);
            }
            trace!("cumulative_q_balance = {cumulative_q_balance}");
            for _ in 0..cumulative_q_balance {
                warn!("Unbalanced q/Q pairs, adding 'Q'");
                content.operations.push(Operation::new("Q", vec![]));
            }
        }

        if log::log_enabled!(log::Level::Trace) {
            for (i, op) in content.operations.iter().enumerate() {
                if i > 20 {
                    break;
                }
                trace!("op {op:?}");
            }
        }
        content_stream.content = content.encode()?;
        content_stream.compress()?;
    }
    Ok(())
}

/// Normalizes a contents array from a source document, deep-copying each stream/reference into dest_doc.
fn normalize_contents_array(
    dest_doc: &mut Document,
    source_doc: &Document,
    items: &[Object],
    copied_objects: &mut BTreeMap<ObjectId, ObjectId>,
) -> Result<Vec<Object>> {
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let id = match item {
            Object::Stream(s) => dest_doc.add_object(s.clone()),
            Object::Reference(id) => {
                pdf_helpers::deep_copy_object_by_id(dest_doc, source_doc, *id, copied_objects)?
            }
            _ => {
                return Err(MedpdfError::new(
                    "Page/Contents array must contain Streams or References",
                ))
            }
        };
        refs.push(Object::Reference(id));
    }
    Ok(refs)
}

/// Resolves a page's `/Contents` value into a normalized `Vec<Object::Reference>`.
///
/// Handles all valid `/Contents` forms: inline Stream, Reference (to Stream or Array), or Array.
/// When `source_doc` is `Some`, streams/references are deep-copied from the source document.
/// When `source_doc` is `None`, the contents are assumed to already reside in `dest_doc`.
fn resolve_contents_to_ref_array(
    dest_doc: &mut Document,
    source_doc: Option<&Document>,
    contents: &Object,
    copied_objects: &mut BTreeMap<ObjectId, ObjectId>,
    context: &str,
) -> Result<Vec<Object>> {
    match contents {
        Object::Stream(stream) => {
            let id = dest_doc.add_object(stream.clone());
            Ok(vec![Object::Reference(id)])
        }
        Object::Reference(reference) => {
            // Clone the resolved object to release the borrow before mutating dest_doc.
            let resolved = if let Some(src) = source_doc {
                src.get_object(*reference)?.clone()
            } else {
                dest_doc.get_object(*reference)?.clone()
            };
            match resolved {
                Object::Stream(stream) => {
                    let id = dest_doc.add_object(stream);
                    Ok(vec![Object::Reference(id)])
                }
                Object::Array(a) => {
                    if let Some(src) = source_doc {
                        normalize_contents_array(dest_doc, src, &a, copied_objects)
                    } else {
                        Ok(a)
                    }
                }
                _ => Err(MedpdfError::Message(format!(
                    "{context} /Contents references a non-stream / non-array"
                ))),
            }
        }
        Object::Array(a) => {
            if let Some(src) = source_doc {
                normalize_contents_array(dest_doc, src, a, copied_objects)
            } else {
                Ok(a.clone())
            }
        }
        _ => Err(MedpdfError::Message(format!(
            "{context} /Contents must be stream or array or reference to stream or array"
        ))),
    }
}

/// Overlays the content of a source page onto a destination page.
pub fn overlay_page(
    dest_doc: &mut Document,
    dest_page_id: ObjectId,
    overlay_doc: &Document,
    overlay_page_num: u32,
) -> Result<()> {
    let overlay_page_id = pdf_helpers::get_page_object_id_from_doc(overlay_doc, overlay_page_num)?;

    let overlay_page = overlay_doc.get_dictionary(overlay_page_id)?;
    let mut copied_objects = BTreeMap::new();

    // Get overlay's Page/Contents, normalizing the structs to be an array of references to
    // Object::Stream(), deep copying the streams from overlay to dest doc.
    // FUTURE: We should just normalize *all* Documents up-front (i.e. all source/input/overlay
    //         documents as well as the destination document).  That would simplify a lot of logic
    //         throughout the code!  (I.e. we could always assume /Resources is a reference and
    //         is nevery anything else, such as an embedded dictionary.  Similar for /Contents.
    //         We could then also handle nested Pages, flattening their /Resources before our
    //         code even sees any of it.  Also, we could centralize removal of stuff we don't or
    //         can't support when merging.  We could also pre-compute hashes for resources, etc.,
    //         to avoid unneeded object duplication (but only of objects we don't modify!).)
    //         Also, normalizing q/Q pairing (enforcing both proper pairing, and that all content
    //         starts with a q and ends with a Q).
    debug!("Standardizing and cloning overlay's /Contents");
    let overlay_contents = overlay_page.get(KEY_CONTENTS)?;
    let overlay_contents_arr_new = resolve_contents_to_ref_array(
        dest_doc, Some(overlay_doc), overlay_contents, &mut copied_objects,
        &format!("Page {overlay_page_id:?}"),
    )?;

    // Generate deep copy of overlay's page's /Resources dictionary, normalizing it to be a
    // reference (rather than an embedded resource).  FUTURE: Also need to copy/merge any parent /Resources...
    debug!("Generating deep copy of overlay's /Resources");
    let overlay_page_resources = overlay_page.get(KEY_RESOURCES)?;
    let overlay_resources_dict_id_new = match overlay_page_resources {
        Object::Dictionary(_) => {
            let d_new = pdf_helpers::deep_copy_object(
                dest_doc,
                overlay_doc,
                overlay_page_resources,
                &mut copied_objects,
            )?;
            dest_doc.add_object(d_new)
        }
        Object::Reference(id) => {
            pdf_helpers::deep_copy_object_by_id(dest_doc, overlay_doc, *id, &mut copied_objects)?
        }
        _ => {
            return Err(MedpdfError::Message(format!(
                "Page {overlay_page_id:?} /Resources must be dictionary or reference to dictionary"
            )))
        }
    };
    // NOTE: As a side-effect of the deep copy of the overlay's /Resources, we've added an unnecessary
    // Object::Dictionary() to the dest_doc... we'll remove this later to tidy up.

    // Starting at the root of the *destination* document, build a list (HashSet) of all keys in
    // all /Resource dictionaries (recursing the full page tree), so we can later make sure no
    // names we add to /Resources conflict!
    debug!("Accumulating dictionary keys in destination document");
    let mut keys_used = HashSet::<Vec<u8>>::new();
    accumulate_dictionary_keys(
        &mut keys_used,
        dest_doc,
        dest_doc.catalog()?.get(KEY_PAGES)?.as_reference()?,
    )?;

    // Now generate new names for all resources in our copied resources_dict_id_new, mutably updating it.
    // Make sure the new names are not present in keys.
    debug!("Renaming keys in overlay dictionaries to be unique in destination");
    let mut key_mapping = HashMap::<Vec<u8>, Vec<u8>>::new();
    rename_resources_in_dict(
        &mut key_mapping,
        &mut keys_used,
        dest_doc,
        overlay_resources_dict_id_new,
    )?;
    // We've now renamed the keys in the Resources dict from the overlay (resources_dict_id_new).
    if log::log_enabled!(log::Level::Trace) {
        trace!("key_mapping:");
        for (k, v) in key_mapping.iter() {
            trace!(
                "{} => {}",
                String::from_utf8_lossy(k),
                String::from_utf8_lossy(v)
            );
        }
    }

    // Update the Contents streams from the overlay document to use the new dictionary keys
    // Unobvious, but changing decoded operations does not modify the Content!!!!  It looks
    // like we need to build a *new* Content, modifying the ops as we copy them.
    debug!("Updating overlay Content streams to use new keys");
    modify_content_stream(dest_doc, &overlay_contents_arr_new, Some(&key_mapping))?;
    if log::log_enabled!(log::Level::Trace) {
        trace!("arr_new: {overlay_contents_arr_new:?}");
        for item in overlay_contents_arr_new.iter() {
            let o = dest_doc.get_object(item.as_reference()?)?;
            trace!("o={o:?}");
        }
    }

    // Now add each element of the Contents array to the destination pages's /Contents (normalizing
    // the destination /Contents to be an array).
    debug!("Merging overlay's /Contents into the destination page's /Contents array");
    // a. We start by getting a copy of dest page's Contents, converting it to an array of
    //    references, if necessary.  Clone the Object to release the immutable borrow on
    //    dest_doc before passing it mutably to the helper.
    let dest_contents = dest_doc
        .get_object(dest_page_id)?
        .as_dict()?
        .get(KEY_CONTENTS)?
        .clone();
    let mut dest_contents_arr_new = resolve_contents_to_ref_array(
        dest_doc, None, &dest_contents, &mut copied_objects,
        &format!("Page {dest_page_id:?}"),
    )?;
    // b. For the original Content, we need to make sure everything is both q/Q balanced, *and*
    //     add a starting q and ending Q to all Content streams!  Otherwise our overlay might be
    //     affected by stray scaling and rotations!
    debug!("Modifying existing Content streams");
    modify_content_stream(dest_doc, &dest_contents_arr_new, None)?;
    // c. We then copy the references from the overlay_contents_arr_new to the end of
    //    dest_content_arr_new.
    for item in overlay_contents_arr_new.iter() {
        let reference = item.as_reference()?;
        dest_contents_arr_new.push(Object::Reference(reference)); // For "underlay": .insert(i, Object::Reference(reference)); where i starts at 0 and increments for each content stream from the underlay.
    }
    if log::log_enabled!(log::Level::Trace) {
        if let Some(first) = overlay_contents_arr_new.first() {
            let obj = dest_doc.get_object(first.as_reference()?)?;
            if let Ok(stream) = obj.as_stream() {
                let ops = stream.decode_content()?.operations;
                trace!("First overlay stream: {} ops", ops.len());
            }
        }
    }
    // c. Finally, we replace the dest page's Content value with our new array.
    let dest_page_dict = dest_doc.get_object_mut(dest_page_id)?.as_dict_mut()?;
    dest_page_dict.set(KEY_CONTENTS, Object::Array(dest_contents_arr_new));

    // On target output page, merge our renamed /Resources with the target page's /Resources dicts
    debug!("Merge overlay's /Resources dictionary (with keys renamed) into destination page's /Resources");
    // a. First, we must ensure the dest page's Resources exists, and normalize it to be a reference
    //    to a separate object.
    //    i. We first determine which scenario we're in: embedded Dictionary or Reference to dict obj:
    let (dict_to_make_object, dict_ref) = match dest_page_dict.get(KEY_RESOURCES) {
        Ok(Object::Dictionary(dict)) => (Some(dict.clone()), None),
        Ok(Object::Reference(reference)) => (None, Some(*reference)),
        Ok(_) => {
            return Err(MedpdfError::new(
                "Destination page's /Resource was not a Dictionary nor Reference",
            ))
        }
        Err(_) => (Some(Dictionary::new()), None),
    };
    //    ii. Now we add a new Dictionary object to dest_doc if needed, or use the one that's already there!
    let dict_ref = match (dict_to_make_object, dict_ref) {
        (Some(dict_to_make_object), None) => {
            dest_doc.add_object(Object::Dictionary(dict_to_make_object))
        }
        (None, Some(dict_ref)) => dict_ref,
        _ => {
            return Err(MedpdfError::new(
                "Internal error: unexpected state in resources normalization",
            ))
        }
    };
    //    iii. Finally, we update dest page's /Resources to be dict_ref
    let dest_page_dict = dest_doc.get_object_mut(dest_page_id)?.as_dict_mut()?;
    dest_page_dict.set(KEY_RESOURCES, Object::Reference(dict_ref));
    // b. Now, we can modify the object pointed to by dict_ref to contain the needed newly renamed
    //    entries from the overlay page's Resources.
    //    i. As a side-effect of deep copying the overlay's Resources, we have added
    //       overlay_resources_dict_id_new, which we won't need in the final document.
    //       We also need to get this "outside" the dest_doc to avoid borrow checker
    //       issues, so we remove this object now, keeping a copy of the underlying
    //       Dictionary, though.
    let source_resources_dict = dest_doc
        .get_object(overlay_resources_dict_id_new)?
        .as_dict()?
        .clone();
    dest_doc.objects.remove(&overlay_resources_dict_id_new);
    //    ii. Now merge the source_resources_dict into the dict_ref
    let dest_resources = dest_doc.get_object_mut(dict_ref)?.as_dict_mut()?;
    // At the root level /Resources dict, each entry is (usually) a dictionary (e.g. /Font, or /XObject)
    // that actually contain the key->value mappings.  We skip /Resources kinds that are not dictionaries
    // (only /ProcSet; see page 83 of https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf)
    for (resource_type, dict) in source_resources_dict.iter() {
        if dest_resources.get(resource_type).is_err() {
            // Key does not exist, so just add entire resource type in one go:
            dest_resources.set(resource_type.clone(), dict.clone());
        } else {
            // Only handle values that are actually dictionaries... there is one /Resource that can be
            // an Array, but we'll skip merging those for now.  (FUTURE!)
            if let Ok(dict) = dict.as_dict() {
                let dest_resource = dest_resources.get_mut(resource_type)?.as_dict_mut()?;
                for (key, value) in dict.iter() {
                    dest_resource.set(key.clone(), value.clone());
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream};

    /// Creates a minimal valid PDF document with one page that has `/Font` and `/XObject` resources.
    fn create_test_pdf_with_resources(
        font_names: &[&str],
        xobject_names: &[&str],
        content_ops: &str,
    ) -> Document {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();

        let mut font_dict = Dictionary::new();
        for name in font_names {
            let font_obj_id = doc.add_object(dictionary! {
                "Type" => "Font",
                "Subtype" => "Type1",
                "BaseFont" => "Helvetica",
            });
            font_dict.set(name.as_bytes(), Object::Reference(font_obj_id));
        }

        let mut xobj_dict = Dictionary::new();
        for name in xobject_names {
            let xobj_id = doc.add_object(Stream::new(dictionary! {}, vec![]));
            xobj_dict.set(name.as_bytes(), Object::Reference(xobj_id));
        }

        let resources = dictionary! {
            "Font" => font_dict,
            "XObject" => xobj_dict,
        };
        let resources_id = doc.add_object(resources);

        let content_stream = Stream::new(dictionary! {}, content_ops.as_bytes().to_vec());
        let content_id = doc.add_object(content_stream);

        let page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => Object::Reference(content_id),
            "Resources" => Object::Reference(resources_id),
        };
        let page_id = doc.add_object(page);

        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => Object::Integer(1),
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        doc
    }

    #[test]
    fn test_find_unique_name_no_collision() {
        let keys = HashSet::new();
        let result = find_unique_name(&keys, b"F1", b"_o").unwrap();
        assert_eq!(result, b"F1_o");
    }

    #[test]
    fn test_find_unique_name_with_collision() {
        let mut keys = HashSet::new();
        keys.insert(b"F1_o".to_vec());
        let result = find_unique_name(&keys, b"F1", b"_o").unwrap();
        assert_eq!(result, b"F1_o1");
    }

    #[test]
    fn test_find_unique_name_multiple_collisions() {
        let mut keys = HashSet::new();
        keys.insert(b"F1_o".to_vec());
        keys.insert(b"F1_o1".to_vec());
        keys.insert(b"F1_o2".to_vec());
        let result = find_unique_name(&keys, b"F1", b"_o").unwrap();
        assert_eq!(result, b"F1_o3");
    }

    #[test]
    fn test_find_unique_name_exhaustion() {
        let mut keys = HashSet::new();
        keys.insert(b"X_o".to_vec());
        for i in 1..10_000 {
            keys.insert(format!("X_o{i}").into_bytes());
        }
        let result = find_unique_name(&keys, b"X", b"_o");
        assert!(result.is_err());
    }

    #[test]
    fn test_overlay_page_basic() {
        let mut dest = create_test_pdf_with_resources(&["F1"], &[], "q\nBT /F1 12 Tf ET\nQ\n");
        let overlay = create_test_pdf_with_resources(&["F2"], &[], "q\nBT /F2 10 Tf ET\nQ\n");

        let dest_page_id = pdf_helpers::get_page_object_id_from_doc(&dest, 1).unwrap();
        overlay_page(&mut dest, dest_page_id, &overlay, 1).unwrap();

        // After overlay, the destination page's Contents should be an array with more streams
        let page_dict = dest.get_dictionary(dest_page_id).unwrap();
        let contents = page_dict.get(KEY_CONTENTS).unwrap();
        let arr = contents.as_array().unwrap();
        assert!(arr.len() >= 2, "Should have multiple content streams after overlay, got {}", arr.len());

        // Verify resources were merged
        let resources_ref = page_dict.get(KEY_RESOURCES).unwrap().as_reference().unwrap();
        let resources = dest.get_dictionary(resources_ref).unwrap();
        let fonts = resources.get(b"Font").unwrap().as_dict().unwrap();
        assert!(fonts.len() >= 2, "Should have merged font resources, got {}", fonts.len());
    }

    #[test]
    fn test_overlay_page_resource_renaming() {
        // Both dest and overlay use the same font name "F1"
        let mut dest = create_test_pdf_with_resources(&["F1"], &[], "q\nBT /F1 12 Tf ET\nQ\n");
        let overlay = create_test_pdf_with_resources(&["F1"], &[], "q\nBT /F1 10 Tf ET\nQ\n");

        let dest_page_id = pdf_helpers::get_page_object_id_from_doc(&dest, 1).unwrap();
        overlay_page(&mut dest, dest_page_id, &overlay, 1).unwrap();

        let page_dict = dest.get_dictionary(dest_page_id).unwrap();
        let resources_ref = page_dict.get(KEY_RESOURCES).unwrap().as_reference().unwrap();
        let resources = dest.get_dictionary(resources_ref).unwrap();
        let fonts = resources.get(b"Font").unwrap().as_dict().unwrap();

        assert!(fonts.has(b"F1"), "Original font F1 should remain");
        assert!(fonts.len() >= 2, "Should have at least 2 font entries after overlay");

        // Verify that at least one key has the "_o" suffix (indicating renaming)
        let has_renamed = fonts.iter().any(|(k, _)| {
            let key_str = String::from_utf8_lossy(k);
            key_str.contains("_o")
        });
        assert!(has_renamed, "Overlay font should have been renamed with _o suffix");
    }

    #[test]
    fn test_overlay_page_preserves_q_balance() {
        // Destination has unbalanced q (extra q without matching Q)
        let mut dest = create_test_pdf_with_resources(&["F1"], &[], "q\nq\nBT /F1 12 Tf ET\nQ\n");
        let overlay = create_test_pdf_with_resources(&["F2"], &[], "q\nBT /F2 10 Tf ET\nQ\n");

        let dest_page_id = pdf_helpers::get_page_object_id_from_doc(&dest, 1).unwrap();
        overlay_page(&mut dest, dest_page_id, &overlay, 1).unwrap();

        // Collect all content stream bytes and count q/Q balance
        let page_dict = dest.get_dictionary(dest_page_id).unwrap();
        let contents = page_dict.get(KEY_CONTENTS).unwrap().as_array().unwrap();
        let mut total_q = 0i32;
        let mut total_big_q = 0i32;
        for item in contents {
            if let Ok(id) = item.as_reference() {
                if let Ok(obj) = dest.get_object(id) {
                    if let Ok(stream) = obj.as_stream() {
                        let bytes = if stream.is_compressed() {
                            stream.decompressed_content().unwrap_or_default()
                        } else {
                            stream.content.clone()
                        };
                        if let Ok(content) = lopdf::content::Content::decode(&bytes) {
                            for op in &content.operations {
                                match op.operator.as_str() {
                                    "q" => total_q += 1,
                                    "Q" => total_big_q += 1,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(total_q, total_big_q, "q/Q should be balanced: q={total_q}, Q={total_big_q}");
    }
}
