//! Low-level PDF helpers: deep object copying, page tree traversal, key constants, and units.

use crate::error::{MedpdfError, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeMap;

pub(crate) const KEY_TYPE: &[u8] = b"Type";
pub(crate) const KEY_PARENT: &[u8] = b"Parent";
pub(crate) const KEY_PAGES: &[u8] = b"Pages";
pub(crate) const KEY_PAGE: &[u8] = b"Page";
pub(crate) const KEY_KIDS: &[u8] = b"Kids";
pub(crate) const KEY_COUNT: &[u8] = b"Count";
pub const KEY_RESOURCES: &[u8] = b"Resources";
pub const KEY_CONTENTS: &[u8] = b"Contents";
pub(crate) const KEY_FONT: &[u8] = b"Font";
pub(crate) const KEY_FONT_DESCRIPTOR: &[u8] = b"FontDescriptor";
pub(crate) const KEY_MEDIA_BOX: &[u8] = b"MediaBox";
pub const KEY_EXTGSTATE: &[u8] = b"ExtGState";
pub const KEY_XOBJECT: &[u8] = b"XObject";

/// Walk the page tree to find the inherited MediaBox for a page.
/// Returns `[x0, y0, x1, y1]` if found on the page or any ancestor node.
pub fn get_page_media_box(doc: &Document, page_id: ObjectId) -> Option<[f32; 4]> {
    let mut current_id = page_id;
    while let Ok(dict) = doc.get_dictionary(current_id) {
        if let Ok(mb) = dict.get(KEY_MEDIA_BOX) {
            if let Ok(arr) = mb.as_array() {
                if arr.len() >= 4 {
                    let x0 = obj_as_f32(&arr[0])?;
                    let y0 = obj_as_f32(&arr[1])?;
                    let x1 = obj_as_f32(&arr[2])?;
                    let y1 = obj_as_f32(&arr[3])?;
                    return Some([x0, y0, x1, y1]);
                }
            }
        }
        // Follow /Parent up the page tree
        match dict.get(KEY_PARENT) {
            Ok(Object::Reference(parent_id)) => current_id = *parent_id,
            _ => break,
        }
    }
    None
}

/// Extract an f32 from a lopdf Object (Integer or Real).
fn obj_as_f32(obj: &Object) -> Option<f32> {
    obj.as_float()
        .ok()
        .or_else(|| obj.as_i64().ok().map(|i| i as f32))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Points (1/72 inch) — the native PDF unit.
    Pt,
    /// Inches (1 inch = 72 points).
    In,
    /// Millimeters (25.4 mm = 72 points).
    Mm,
    /// Centimeters (2.54 cm = 72 points).
    Cm,
}

impl Unit {
    pub fn to_points(self, value: f32) -> f32 {
        const POINTS_PER_INCH: f32 = 72.0;
        const POINTS_PER_MM: f32 = POINTS_PER_INCH / 25.4;
        const POINTS_PER_CM: f32 = POINTS_PER_INCH / 2.54;
        match self {
            Unit::Pt => value,
            Unit::In => value * POINTS_PER_INCH,
            Unit::Mm => value * POINTS_PER_MM,
            Unit::Cm => value * POINTS_PER_CM,
        }
    }
}

pub(crate) const KEY_ROTATE: &[u8] = b"Rotate";

/// Gets the rotation angle of a page in degrees (0, 90, 180, or 270).
/// Walks the page tree upward to find inherited `/Rotate` values.
/// Returns 0 if no `/Rotate` entry is found.
pub fn get_page_rotation(doc: &Document, page_id: ObjectId) -> u32 {
    let mut current_id = page_id;
    while let Ok(dict) = doc.get_dictionary(current_id) {
        if let Ok(obj) = dict.get(KEY_ROTATE) {
            if let Ok(val) = obj.as_i64() {
                // Normalize to 0/90/180/270
                return ((val % 360 + 360) % 360) as u32;
            }
        }
        match dict.get(KEY_PARENT) {
            Ok(Object::Reference(parent_id)) => current_id = *parent_id,
            _ => break,
        }
    }
    0
}

/// Sets the rotation angle on a page. Valid values: 0, 90, 180, 270.
/// Returns an error if the angle is not a multiple of 90.
pub fn set_page_rotation(doc: &mut Document, page_id: ObjectId, degrees: u32) -> Result<()> {
    if !degrees.is_multiple_of(90) {
        return Err(MedpdfError::new(format!(
            "Rotation must be a multiple of 90, got {degrees}"
        )));
    }
    let normalized = degrees % 360;
    let page = doc.get_object_mut(page_id)?.as_dict_mut()?;
    if normalized == 0 {
        page.remove(KEY_ROTATE);
    } else {
        page.set(KEY_ROTATE, Object::Integer(normalized as i64));
    }
    Ok(())
}

/// Gets the object ID of a page from a document.
/// Note: This calls `doc.get_pages()` which walks the entire page tree each time.
/// For bulk operations, callers should cache the result of `doc.get_pages()` externally.
pub(crate) fn get_page_object_id_from_doc(doc: &Document, page_num: u32) -> Result<ObjectId> {
    doc.get_pages().get(&page_num).copied().ok_or_else(|| {
        MedpdfError::new(format!("Page {} not found in source document", page_num))
    })
}

pub fn deep_copy_object_by_id(
    dest_doc: &mut Document,
    source_doc: &Document,
    source_object_id: ObjectId,
    copied_objects: &mut BTreeMap<ObjectId, ObjectId>, // maps source_object_id to dest object_id
) -> Result<ObjectId> {
    if let Some(&new_id) = copied_objects.get(&source_object_id) {
        return Ok(new_id);
    }

    let new_obj = deep_copy_object(
        dest_doc,
        source_doc,
        source_doc.get_object(source_object_id)?,
        copied_objects,
    )?;
    let new_id = dest_doc.add_object(new_obj);
    copied_objects.insert(source_object_id, new_id);
    Ok(new_id)
}

pub fn deep_copy_object(
    dest_doc: &mut Document,
    source_doc: &Document,
    source_object: &Object,
    copied_objects: &mut BTreeMap<ObjectId, ObjectId>, // maps source_object_id to dest object_id
) -> Result<Object> {
    let new_obj = match source_object {
        Object::Reference(_) => {
            return Err(MedpdfError::new(
                "deep_copy_object() called on a Object::Reference!",
            ));
        }
        Object::Dictionary(source_dict) => {
            let mut dest_dict = Dictionary::new();
            for (key, value) in source_dict.iter() {
                if key == KEY_PARENT {
                    continue;
                } // We never want to deep copy *up* the tree, as we'll then copy the whole document!
                if let Object::Reference(id) = value {
                    dest_dict.set(
                        key.clone(),
                        Object::Reference(deep_copy_object_by_id(
                            dest_doc,
                            source_doc,
                            *id,
                            copied_objects,
                        )?),
                    );
                } else {
                    dest_dict.set(
                        key.clone(),
                        deep_copy_object(dest_doc, source_doc, value, copied_objects)?,
                    );
                }
            }
            Object::Dictionary(dest_dict)
        }
        Object::Array(source_arr) => {
            let mut dest_arr = Vec::<Object>::with_capacity(source_arr.len());
            for item in source_arr.iter() {
                if let Object::Reference(id) = item {
                    dest_arr.push(Object::Reference(deep_copy_object_by_id(
                        dest_doc,
                        source_doc,
                        *id,
                        copied_objects,
                    )?));
                } else {
                    dest_arr.push(deep_copy_object(
                        dest_doc,
                        source_doc,
                        item,
                        copied_objects,
                    )?)
                }
            }
            Object::Array(dest_arr)
        }
        Object::Stream(source_stream) => {
            let source_dict = &source_stream.dict;
            let source_content = &source_stream.content;

            let mut dest_dict = Dictionary::new();
            for (key, value) in source_dict.iter() {
                if let Object::Reference(id) = value {
                    dest_dict.set(
                        key.clone(),
                        Object::Reference(deep_copy_object_by_id(
                            dest_doc,
                            source_doc,
                            *id,
                            copied_objects,
                        )?),
                    );
                } else {
                    dest_dict.set(
                        key.clone(),
                        deep_copy_object(dest_doc, source_doc, value, copied_objects)?,
                    );
                }
            }

            let new_stream = Stream::new(dest_dict, source_content.clone());
            Object::Stream(new_stream)
        }
        _ => source_object.clone(),
    };

    Ok(new_obj)
}

/// Registers an object reference in a page's Resources sub-dictionary.
///
/// Handles the three-tier pattern: Page dict -> Resources (inline or ref) -> sub-dict (inline or ref).
/// `resource_key` selects the sub-dictionary (e.g. `KEY_FONT`, `KEY_EXTGSTATE`, `KEY_XOBJECT`).
/// `entry_key` is the name under which the reference is stored (e.g. `b"F7"`, `b"GS7"`, `b"Img1"`).
pub fn register_in_page_resources(
    doc: &mut Document,
    page_id: ObjectId,
    resource_key: &[u8],
    entry_key: &[u8],
    obj_id: ObjectId,
) -> Result<()> {
    // First pass: handle page's Resources (may be inline dict or reference)
    let page_dict = doc
        .get_object_mut(page_id)?
        .as_dict_mut()
        .map_err(|_| MedpdfError::new("Page object is not a dictionary"))?;

    let resources_obj = page_dict.get_mut(KEY_RESOURCES);
    let (mut subdict_ref_id, resources_dict_id) = match resources_obj {
        Ok(Object::Reference(id)) => (None, Some(*id)),
        Ok(Object::Dictionary(dict)) => {
            let r = handle_subdict_in_resources(dict, resource_key, entry_key, obj_id)?;
            (r, None)
        }
        Ok(_) => {
            return Err(MedpdfError::new(
                "/Resources key of page not a Reference nor a Dictionary!",
            ))
        }
        Err(_) => {
            let mut dict = dictionary! {};
            let r = handle_subdict_in_resources(&mut dict, resource_key, entry_key, obj_id)?;
            page_dict.set(KEY_RESOURCES, Object::Dictionary(dict));
            (r, None)
        }
    };

    if subdict_ref_id.is_some() && resources_dict_id.is_some() {
        return Err(MedpdfError::new(
            "Internal error: both sub-dict ref and Resources ref are set!",
        ));
    }

    // Second pass: if Resources was a reference, handle it now
    if let Some(res_id) = resources_dict_id {
        let res_dict = doc.get_object_mut(res_id)?.as_dict_mut()?;
        subdict_ref_id = handle_subdict_in_resources(res_dict, resource_key, entry_key, obj_id)?;
    }

    // Third pass: if the sub-dictionary was a reference, add entry directly
    if let Some(subdict_id) = subdict_ref_id {
        let subdict = doc.get_object_mut(subdict_id)?.as_dict_mut()?;
        subdict.set(entry_key.to_vec(), Object::Reference(obj_id));
    }

    Ok(())
}

/// Handles adding an entry to a Resources sub-dictionary.
/// Returns `Some(ObjectId)` if the sub-dictionary is a reference that needs separate handling.
fn handle_subdict_in_resources(
    resources_dict: &mut Dictionary,
    resource_key: &[u8],
    entry_key: &[u8],
    obj_id: ObjectId,
) -> Result<Option<ObjectId>> {
    match resources_dict.get_mut(resource_key) {
        Ok(Object::Reference(id)) => Ok(Some(*id)),
        Ok(Object::Dictionary(dict)) => {
            dict.set(entry_key.to_vec(), Object::Reference(obj_id));
            Ok(None)
        }
        Ok(_) => Err(MedpdfError::new(format!(
            "/{} key of Resources not a Reference nor a Dictionary!",
            String::from_utf8_lossy(resource_key)
        ))),
        Err(_) => {
            let mut dict = dictionary! {};
            dict.set(entry_key.to_vec(), Object::Reference(obj_id));
            resources_dict.set(resource_key.to_vec(), Object::Dictionary(dict));
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream};

    /// Creates a minimal valid PDF document with no pages.
    fn create_empty_pdf() -> Document {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![],
            "Count" => 0,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc
    }

    /// Creates a minimal valid PDF with the specified number of pages.
    fn create_pdf_with_pages(count: usize) -> Document {
        let mut doc = create_empty_pdf();
        let pages_id = doc
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();

        for _ in 0..count {
            let media_box = vec![0.0.into(), 0.0.into(), 612.0.into(), 792.0.into()];
            let resources_id = doc.add_object(dictionary! {});
            let content_id = doc.add_object(Stream::new(dictionary! {}, vec![]));
            let page = dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => media_box,
                "Contents" => Object::Reference(content_id),
                "Resources" => Object::Reference(resources_id),
            };
            let page_id = doc.add_object(page);
            let pages = doc.get_object_mut(pages_id).unwrap().as_dict_mut().unwrap();
            let kids = pages.get_mut(b"Kids").unwrap().as_array_mut().unwrap();
            kids.push(page_id.into());
            let new_count = kids.len();
            pages.set("Count", Object::Integer(new_count as i64));
        }
        doc
    }

    // --- get_page_object_id_from_doc tests ---

    #[test]
    fn test_get_page_object_id_single_page() {
        let doc = create_pdf_with_pages(1);
        let result = get_page_object_id_from_doc(&doc, 1);
        assert!(result.is_ok());
        let page_id = result.unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        assert_eq!(page.get(b"Type").unwrap().as_name().unwrap(), b"Page");
    }

    #[test]
    fn test_get_page_object_id_multiple_pages() {
        let doc = create_pdf_with_pages(5);
        for page_num in 1..=5 {
            let result = get_page_object_id_from_doc(&doc, page_num);
            assert!(result.is_ok(), "Failed to get page {}", page_num);
        }
    }

    #[test]
    fn test_get_page_object_id_different_pages_different_ids() {
        let doc = create_pdf_with_pages(3);
        let id1 = get_page_object_id_from_doc(&doc, 1).unwrap();
        let id2 = get_page_object_id_from_doc(&doc, 2).unwrap();
        let id3 = get_page_object_id_from_doc(&doc, 3).unwrap();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_get_page_object_id_page_not_found() {
        let doc = create_pdf_with_pages(3);
        let result = get_page_object_id_from_doc(&doc, 4);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_page_object_id_page_zero() {
        let doc = create_pdf_with_pages(3);
        let result = get_page_object_id_from_doc(&doc, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_page_object_id_empty_doc() {
        let doc = create_empty_pdf();
        let result = get_page_object_id_from_doc(&doc, 1);
        assert!(result.is_err());
    }

    // --- deep_copy_object tests ---

    #[test]
    fn test_deep_copy_integer() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Integer(42);
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Object::Integer(42));
    }

    #[test]
    fn test_deep_copy_real() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Real(3.15);
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Real(v) = result.unwrap() {
            assert!((v - 3.15).abs() < 0.001);
        } else {
            panic!("Expected Real object");
        }
    }

    #[test]
    fn test_deep_copy_string() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::String(b"Hello".to_vec(), lopdf::StringFormat::Literal);
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::String(bytes, _) = result.unwrap() {
            assert_eq!(bytes, b"Hello".to_vec());
        } else {
            panic!("Expected String object");
        }
    }

    #[test]
    fn test_deep_copy_name() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Name(b"TestName".to_vec());
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Name(bytes) = result.unwrap() {
            assert_eq!(bytes, b"TestName".to_vec());
        } else {
            panic!("Expected Name object");
        }
    }

    #[test]
    fn test_deep_copy_boolean_true() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &Object::Boolean(true), &mut copied);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Object::Boolean(true));
    }

    #[test]
    fn test_deep_copy_boolean_false() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &Object::Boolean(false), &mut copied);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Object::Boolean(false));
    }

    #[test]
    fn test_deep_copy_null() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &Object::Null, &mut copied);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Object::Null);
    }

    #[test]
    fn test_deep_copy_array() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Array(arr) = result.unwrap() {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Object::Integer(1));
            assert_eq!(arr[1], Object::Integer(2));
            assert_eq!(arr[2], Object::Integer(3));
        } else {
            panic!("Expected Array object");
        }
    }

    #[test]
    fn test_deep_copy_nested_array() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Array(vec![
            Object::Array(vec![Object::Integer(1), Object::Integer(2)]),
            Object::Array(vec![Object::Integer(3), Object::Integer(4)]),
        ]);
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Array(arr) = result.unwrap() {
            assert_eq!(arr.len(), 2);
            if let Object::Array(inner) = &arr[0] {
                assert_eq!(inner.len(), 2);
            } else {
                panic!("Expected nested array");
            }
        } else {
            panic!("Expected Array object");
        }
    }

    #[test]
    fn test_deep_copy_dictionary() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Dictionary(dictionary! {
            "Key1" => 42,
            "Key2" => "value",
        });
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Dictionary(dict) = result.unwrap() {
            assert_eq!(dict.get(b"Key1").unwrap(), &Object::Integer(42));
        } else {
            panic!("Expected Dictionary object");
        }
    }

    #[test]
    fn test_deep_copy_dictionary_skips_parent() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference((1, 0)),
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
        });
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Dictionary(dict) = result.unwrap() {
            assert!(dict.get(b"Parent").is_err(), "Parent key should be skipped during copy");
            assert!(dict.get(b"Type").is_ok());
            assert!(dict.get(b"MediaBox").is_ok());
        } else {
            panic!("Expected Dictionary object");
        }
    }

    #[test]
    fn test_deep_copy_stream() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Stream(Stream::new(dictionary! {}, b"q 1 0 0 1 0 0 cm Q".to_vec()));
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Stream(stream) = result.unwrap() {
            assert_eq!(stream.content, b"q 1 0 0 1 0 0 cm Q".to_vec());
        } else {
            panic!("Expected Stream object");
        }
    }

    #[test]
    fn test_deep_copy_reference_error() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Reference((1, 0));
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_err());
    }

    // --- deep_copy_object_by_id tests ---

    #[test]
    fn test_deep_copy_by_id_caches_result() {
        let mut dest_doc = create_empty_pdf();
        let mut source_doc = create_empty_pdf();
        let source_id = source_doc.add_object(Object::Integer(42));
        let mut copied = BTreeMap::new();

        let dest_id1 = deep_copy_object_by_id(&mut dest_doc, &source_doc, source_id, &mut copied).unwrap();
        let dest_id2 = deep_copy_object_by_id(&mut dest_doc, &source_doc, source_id, &mut copied).unwrap();
        assert_eq!(dest_id1, dest_id2, "Same source object should map to same dest ID");
    }

    #[test]
    fn test_deep_copy_by_id_different_objects_different_ids() {
        let mut dest_doc = create_empty_pdf();
        let mut source_doc = create_empty_pdf();
        let source_id1 = source_doc.add_object(Object::Integer(1));
        let source_id2 = source_doc.add_object(Object::Integer(2));
        let mut copied = BTreeMap::new();

        let dest_id1 = deep_copy_object_by_id(&mut dest_doc, &source_doc, source_id1, &mut copied).unwrap();
        let dest_id2 = deep_copy_object_by_id(&mut dest_doc, &source_doc, source_id2, &mut copied).unwrap();
        assert_ne!(dest_id1, dest_id2, "Different source objects should have different dest IDs");
    }

    #[test]
    fn test_deep_copy_by_id_missing_source_object() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let bogus_id = (9999, 0);
        let result = deep_copy_object_by_id(&mut dest_doc, &source_doc, bogus_id, &mut copied);
        assert!(result.is_err(), "Missing source object should fail");
    }

    // --- deep_copy with references ---

    #[test]
    fn test_deep_copy_dictionary_with_reference_values() {
        let mut dest_doc = create_empty_pdf();
        let mut source_doc = create_empty_pdf();
        let child_id = source_doc.add_object(Object::Integer(999));
        let source_obj = Object::Dictionary(dictionary! {
            "Type" => "Test",
            "Child" => Object::Reference(child_id),
        });
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Dictionary(dict) = result.unwrap() {
            let child_ref = dict.get(b"Child").unwrap();
            if let Object::Reference(new_id) = child_ref {
                let obj = dest_doc.get_object(*new_id).unwrap();
                assert_eq!(*obj, Object::Integer(999));
            } else {
                panic!("Child should still be a reference");
            }
        } else {
            panic!("Expected Dictionary");
        }
    }

    #[test]
    fn test_deep_copy_array_with_reference_elements() {
        let mut dest_doc = create_empty_pdf();
        let mut source_doc = create_empty_pdf();
        let child_id = source_doc.add_object(Object::Integer(42));
        let source_obj = Object::Array(vec![
            Object::Integer(1),
            Object::Reference(child_id),
            Object::Integer(3),
        ]);
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Array(arr) = result.unwrap() {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Object::Integer(1));
            if let Object::Reference(new_id) = &arr[1] {
                let obj = dest_doc.get_object(*new_id).unwrap();
                assert_eq!(*obj, Object::Integer(42));
            } else {
                panic!("Expected reference in array");
            }
            assert_eq!(arr[2], Object::Integer(3));
        } else {
            panic!("Expected Array");
        }
    }

    #[test]
    fn test_deep_copy_stream_with_dict_references() {
        let mut dest_doc = create_empty_pdf();
        let mut source_doc = create_empty_pdf();
        let font_id = source_doc.add_object(dictionary! {
            "Type" => "Font",
            "BaseFont" => "Helvetica",
        });
        let stream_dict = dictionary! {
            "Font" => Object::Reference(font_id),
        };
        let source_obj = Object::Stream(Stream::new(stream_dict, b"test content".to_vec()));
        let mut copied = BTreeMap::new();
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Stream(stream) = result.unwrap() {
            assert_eq!(stream.content, b"test content");
            let font_ref = stream.dict.get(b"Font").unwrap();
            if let Object::Reference(new_id) = font_ref {
                let font_obj = dest_doc.get_object(*new_id).unwrap();
                assert!(font_obj.as_dict().is_ok());
            } else {
                panic!("Font should be a reference");
            }
        } else {
            panic!("Expected Stream");
        }
    }

    #[test]
    fn test_deep_copy_nested_dictionary() {
        let mut dest_doc = create_empty_pdf();
        let source_doc = create_empty_pdf();
        let mut copied = BTreeMap::new();
        let source_obj = Object::Dictionary(dictionary! {
            "Outer" => dictionary! {
                "Inner" => 42,
            },
        });
        let result = deep_copy_object(&mut dest_doc, &source_doc, &source_obj, &mut copied);
        assert!(result.is_ok());
        if let Object::Dictionary(outer) = result.unwrap() {
            if let Object::Dictionary(inner) = outer.get(b"Outer").unwrap() {
                assert_eq!(inner.get(b"Inner").unwrap(), &Object::Integer(42));
            } else {
                panic!("Expected nested dictionary");
            }
        } else {
            panic!("Expected Dictionary");
        }
    }
}
