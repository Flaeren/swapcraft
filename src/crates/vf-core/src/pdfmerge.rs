//! Joining PDFs with bookmarks and cutting a page box, in-process with `lopdf`, for the sheets the CAD
//! engine writes (one small PDF per layout).

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{Bookmark, Dictionary, Document, Object, ObjectId};

/// Page attributes a page may inherit from its `Pages` ancestors. The merged file has one flat `Pages`
/// node, so they are copied onto each page first.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn flatten_inherited(doc: &mut Document) {
    let ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for id in ids {
        let mut found: Vec<(Vec<u8>, Object)> = Vec::new();
        for key in INHERITED {
            let mut cur = id;
            // a page tree is shallow; the bound only protects against a malformed loop
            for _ in 0..32 {
                let Ok(dict) = doc.get_dictionary(cur) else { break };
                if let Ok(v) = dict.get(key) {
                    if cur != id {
                        found.push((key.to_vec(), v.clone()));
                    }
                    break;
                }
                match dict.get(b"Parent").and_then(Object::as_reference) {
                    Ok(parent) => cur = parent,
                    Err(_) => break,
                }
            }
        }
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(id) {
            for (k, v) in found {
                page.set(k, v);
            }
        }
    }
}

/// Joins the PDFs in `parts` (bookmark title, file), in order, into `out`. Each bookmark points at the
/// first page of its part.
pub fn merge_with_bookmarks(parts: &[(String, &Path)], out: &Path) -> Result<(), String> {
    let mut max_id = 1;
    let mut pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut merged = Document::with_version("1.5");
    for (title, path) in parts {
        let mut doc = Document::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        flatten_inherited(&mut doc);
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        let mut first = None;
        for id in doc.get_pages().into_values() {
            first.get_or_insert(id);
            let page = doc.get_object(id).map_err(|e| e.to_string())?.to_owned();
            pages.insert(id, page);
        }
        merged.add_bookmark(Bookmark::new(title.clone(), [0.0, 0.0, 0.0], 0, first.unwrap_or((0, 0))), None);
        objects.extend(doc.objects);
    }

    let mut catalog: Option<(ObjectId, Object)> = None;
    let mut pages_node: Option<(ObjectId, Object)> = None;
    for (id, object) in objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => {
                catalog = Some((catalog.as_ref().map(|(i, _)| *i).unwrap_or(id), object));
            }
            b"Pages" => {
                if let Ok(dict) = object.as_dict() {
                    let mut dict = dict.clone();
                    if let Some((_, old)) = &pages_node
                        && let Ok(old) = old.as_dict()
                    {
                        dict.extend(old);
                    }
                    pages_node = Some((pages_node.as_ref().map(|(i, _)| *i).unwrap_or(id), Object::Dictionary(dict)));
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                merged.objects.insert(id, object);
            }
        }
    }
    let (pages_id, pages_obj) = pages_node.ok_or("no page tree in the sheets")?;
    let (catalog_id, catalog_obj) = catalog.ok_or("no catalog in the sheets")?;

    for (id, object) in &pages {
        if let Ok(dict) = object.as_dict() {
            let mut dict = dict.clone();
            dict.set("Parent", pages_id);
            merged.objects.insert(*id, Object::Dictionary(dict));
        }
    }
    let mut node: Dictionary = pages_obj.as_dict().map_err(|e| e.to_string())?.clone();
    node.set("Count", pages.len() as u32);
    node.set("Kids", pages.keys().map(|k| Object::Reference(*k)).collect::<Vec<_>>());
    merged.objects.insert(pages_id, Object::Dictionary(node));

    let mut cat: Dictionary = catalog_obj.as_dict().map_err(|e| e.to_string())?.clone();
    cat.set("Pages", pages_id);
    cat.set("PageMode", "UseOutlines");
    cat.remove(b"Outlines");
    merged.objects.insert(catalog_id, Object::Dictionary(cat));
    merged.trailer.set("Root", catalog_id);
    merged.max_id = merged.objects.len() as u32;
    merged.renumber_objects();
    merged.adjust_zero_pages();
    if let Some(outline) = merged.build_outline()
        && let Ok(Object::Dictionary(cat)) = merged.get_object_mut(catalog_id)
    {
        cat.set("Outlines", Object::Reference(outline));
    }
    merged.save(out).map_err(|e| e.to_string())?;
    Ok(())
}

/// Writes `src` to `out` with page 1's MediaBox and CropBox set to `rect` (`[x0, y0, x1, y1]`, points,
/// origin at the bottom left) and every other page dropped. The content is not touched, so the page
/// keeps its vector data and its size on disk.
pub fn crop_first_page(src: &Path, out: &Path, rect: [f64; 4]) -> Result<(), String> {
    let mut doc = Document::load(src).map_err(|e| format!("{}: {e}", src.display()))?;
    flatten_inherited(&mut doc);
    let pages = doc.get_pages();
    let first = *pages.values().next().ok_or("the export has no page")?;
    for (n, _) in pages.iter().skip(1) {
        doc.delete_pages(&[*n]);
    }
    let arr = |r: [f64; 4]| Object::Array(r.iter().map(|v| Object::Real(*v as f32)).collect());
    if let Ok(Object::Dictionary(page)) = doc.get_object_mut(first) {
        page.set("MediaBox", arr(rect));
        page.set("CropBox", arr(rect));
    }
    doc.save(out).map_err(|e| e.to_string())?;
    Ok(())
}
