//! Page deletion from PDF documents.

use crate::{
    error::{MedpdfError, Result},
    pdf_helpers::{get_page_object_id_from_doc, KEY_COUNT, KEY_KIDS},
};
use lopdf::{Document, Object, ObjectId};

/// Removes a page from the document by 1-based page number.
///
/// Updates the parent `/Pages` node's `/Kids` array and `/Count`.
/// The page object and its resources are left in the document's object table
/// (lopdf does not garbage-collect unreferenced objects until save).
///
/// Returns the `ObjectId` of the removed page.
pub fn delete_page(doc: &mut Document, page_num: u32) -> Result<ObjectId> {
    let page_id = get_page_object_id_from_doc(doc, page_num)?;

    // Find the parent Pages node for this page
    let page_dict = doc.get_dictionary(page_id)?;
    let parent_id = page_dict
        .get(b"Parent")
        .map_err(|_| MedpdfError::new("Page has no /Parent reference"))?
        .as_reference()
        .map_err(|_| MedpdfError::new("Page /Parent is not a reference"))?;

    // Remove from /Kids and update /Count
    let parent = doc.get_object_mut(parent_id)?.as_dict_mut()?;
    let kids = parent.get_mut(KEY_KIDS)?.as_array_mut()?;

    let original_len = kids.len();
    kids.retain(|obj| {
        if let Object::Reference(id) = obj {
            *id != page_id
        } else {
            true
        }
    });

    if kids.len() == original_len {
        return Err(MedpdfError::new(format!(
            "Page {page_id:?} not found in parent's /Kids array"
        )));
    }

    let new_count = kids.len();
    parent.set(KEY_COUNT, Object::Integer(new_count as i64));

    Ok(page_id)
}
