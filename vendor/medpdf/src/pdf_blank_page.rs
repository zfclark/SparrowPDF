//! Blank page creation.

use crate::{
    error::Result,
    pdf_helpers::{KEY_COUNT, KEY_KIDS, KEY_PAGES, KEY_PARENT},
};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};

/// Creates a new, blank page with the specified dimensions and adds it to the document.
pub fn create_blank_page(dest_doc: &mut Document, width: f32, height: f32) -> Result<ObjectId> {
    let media_box = vec![0.0.into(), 0.0.into(), width.into(), height.into()];
    let resources_id = dest_doc.add_object(dictionary! {});
    let content_id = dest_doc.add_object(Stream::new(dictionary! {}, vec![]));

    let page = dictionary! {
        "Type" => "Page",
        "MediaBox" => media_box,
        "Contents" => Object::Reference(content_id),
        "Resources" => Object::Reference(resources_id),
    };
    let page_id = dest_doc.add_object(page);

    let pages_id = dest_doc.catalog()?.get(KEY_PAGES)?.as_reference()?;

    // Add page to Kids array
    let pages = dest_doc.get_object_mut(pages_id)?.as_dict_mut()?;
    let kids = pages.get_mut(KEY_KIDS)?.as_array_mut()?;
    kids.push(page_id.into());

    // Update page count
    let new_page_count = kids.len();
    pages.set(KEY_COUNT, Object::Integer(new_page_count as i64));

    // Set Parent for the new page
    let page_object = dest_doc.get_object_mut(page_id)?.as_dict_mut()?;
    page_object.set(KEY_PARENT, Object::Reference(pages_id));

    Ok(page_id)
}
