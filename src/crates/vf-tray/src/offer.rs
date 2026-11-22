//! What the drop bar offers for the files being dragged: the formats every one of them can become, taken from the
//! same route table the hub, the command line and the right-click menu use (`vf_core::routes`), so the bar can
//! never offer a conversion the rest of SwapCraft would not do, nor the format a file already has.

use std::path::PathBuf;

use vf_core::routes;

/// One format on the bar: the route it runs, the words on it and the file type it writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// `routes::route_id`: `pdf`, `docx`, `png`... Empty for the "More" slot.
    pub id: String,
    pub label: String,
    /// The extension the route writes (`pdf`), or empty when it writes a folder or this is the "More" slot.
    pub ext: String,
    /// False when this PC lacks the engine the conversion needs (WordCraft, PdfCraft...): the bar shows the format
    /// dimmed and nothing can be dropped on it.
    pub available: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offer {
    /// What the bar shows; the last one is the "More" slot when `more` is not empty.
    pub slots: Vec<Slot>,
    /// The formats behind "More".
    pub more: Vec<Slot>,
}

/// Slots shown before "More" takes over.
const SHOWN: usize = 3;

pub const MORE_LABEL: &str = "Altro\u{2026}";

impl Offer {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether slot `i` is the "More" slot.
    pub fn is_more_slot(&self, i: usize) -> bool {
        !self.more.is_empty() && i + 1 == self.slots.len()
    }

    /// The format a hot id stands for: a main slot index, or `CHIP_BASE + k` for the k-th of `more`.
    pub fn pick(&self, hot: i32) -> Option<&Slot> {
        let slot = if hot >= CHIP_BASE {
            self.more.get((hot - CHIP_BASE) as usize)
        } else if hot >= 0 && !self.is_more_slot(hot as usize) {
            self.slots.get(hot as usize)
        } else {
            None
        };
        slot.filter(|s| s.available)
    }
}

/// Hot ids from here on are the formats of the opened "More" list; below are the main slots.
pub const CHIP_BASE: i32 = 100;

/// The file types of `paths` (lower case, no dot; a file with no extension counts by its content), sorted, once each.
pub fn exts_of(paths: &[PathBuf]) -> Vec<String> {
    let mut v: Vec<String> = paths.iter().filter(|p| p.is_file()).map(|p| routes::ext_of(p)).filter(|e| !e.is_empty()).collect();
    v.sort();
    v.dedup();
    v
}

/// Every format all of `exts` can become, in the route table's order. Empty when `exts` is empty or one of the
/// types has no conversion at all.
pub fn common(exts: &[String]) -> Vec<Slot> {
    let Some(first) = exts.first() else { return Vec::new() };
    let mut out: Vec<Slot> = Vec::new();
    for &i in &routes::targets(first) {
        let route = &routes::ROUTES[i];
        let id = routes::route_id(route);
        if out.iter().any(|s| s.id == id) || !exts.iter().all(|e| routes::find_route(e, &id).is_some()) {
            continue;
        }
        let available = exts.iter().all(|e| routes::find_route(e, &id).is_some_and(|r| routes::available(r, e)));
        out.push(Slot { id, label: routes::shown_label(route.label), ext: routes::output_ext(route).unwrap_or("").to_owned(), available });
    }
    out
}

/// The bar's contents for files of these types: up to four formats, or three and "More" with the rest behind it.
pub fn offer(exts: &[String]) -> Offer {
    let mut slots = common(exts);
    if slots.len() <= SHOWN + 1 {
        return Offer { slots, more: Vec::new() };
    }
    let more = slots.split_off(SHOWN);
    // "More" itself is open as long as one of the formats behind it can be made
    let available = more.iter().any(|s| s.available);
    slots.push(Slot { id: String::new(), label: MORE_LABEL.to_owned(), ext: String::new(), available });
    Offer { slots, more }
}

/// What the bar shows while the files being dragged are not known yet (reading the selection takes a moment, and
/// some windows cannot be read): the formats most files can become. The data object of the drag corrects it as
/// soon as the cursor enters the bar, before anything can be dropped.
pub fn generic() -> Offer {
    let slot = |id: &str, label: &str| Slot { id: id.to_owned(), label: label.to_owned(), ext: id.to_owned(), available: true };
    Offer { slots: vec![slot("pdf", "PDF"), slot("docx", "Word (DOCX)"), slot("png", "PNG"), slot("txt", "Text (TXT)")], more: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(slots: &[Slot]) -> Vec<&str> {
        slots.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn a_pdf_is_never_offered_as_a_pdf() {
        let o = offer(&["pdf".to_owned()]);
        let all: Vec<&Slot> = o.slots.iter().chain(&o.more).collect();
        assert!(!all.is_empty());
        assert!(all.iter().all(|s| s.id != "pdf"), "{all:?}");
        assert!(all.iter().any(|s| s.id == "docx"), "{all:?}");
    }

    #[test]
    fn a_spreadsheet_offers_pdf_xlsx_and_csv_but_not_its_own_format() {
        let o = offer(&["xlsx".to_owned()]);
        assert!(ids(&o.slots).contains(&"pdf"), "{o:?}");
        assert!(!ids(&o.slots).contains(&"xlsx"), "{o:?}");
        assert!(o.more.is_empty(), "three formats fit without More: {o:?}");
    }

    #[test]
    fn a_picture_gets_three_formats_and_more() {
        let o = offer(&["png".to_owned()]);
        assert_eq!(o.slots.len(), 4, "{o:?}");
        assert_eq!(o.slots[3].label, MORE_LABEL);
        assert!(o.is_more_slot(3) && !o.is_more_slot(2));
        assert!(!o.more.is_empty());
        let all: Vec<&Slot> = o.slots.iter().chain(&o.more).collect();
        assert!(all.iter().all(|s| s.id != "png"), "{all:?}");
        // a click on More itself picks nothing
        assert!(o.pick(3).is_none());
        // (a format whose engine is not on this PC is not picked: see `a_format_without_its_engine_cannot_be_picked`)
        assert_eq!(o.pick(CHIP_BASE).map(|s| s.id.as_str()), Some(o.more[0].id.as_str()).filter(|_| o.more[0].available));
        assert_eq!(o.pick(0).map(|s| s.id.as_str()), Some(o.slots[0].id.as_str()).filter(|_| o.slots[0].available));
    }

    #[test]
    fn a_format_without_its_engine_cannot_be_picked() {
        let slot = |id: &str, available: bool| Slot { id: id.to_owned(), label: id.to_uppercase(), ext: id.to_owned(), available };
        let o = Offer { slots: vec![slot("pdf", false), slot("docx", true), Slot { id: String::new(), label: MORE_LABEL.to_owned(), ext: String::new(), available: true }], more: vec![slot("txt", false), slot("html", true)] };
        assert!(o.pick(0).is_none(), "PDF needs an engine this PC lacks");
        assert_eq!(o.pick(1).map(|s| s.id.as_str()), Some("docx"));
        assert!(o.pick(CHIP_BASE).is_none());
        assert_eq!(o.pick(CHIP_BASE + 1).map(|s| s.id.as_str()), Some("html"));
    }

    #[test]
    fn availability_follows_the_engines_of_this_pc() {
        use vf_core::engine::{PDFCRAFT, WORDCRAFT};
        let o = offer(&["docx".to_owned()]);
        let pdf = o.slots.iter().chain(&o.more).find(|s| s.id == "pdf").expect("a Word file can become a PDF");
        assert_eq!(pdf.available, WORDCRAFT.find().is_some());
        let o = offer(&["pdf".to_owned()]);
        let docx = o.slots.iter().chain(&o.more).find(|s| s.id == "docx").expect("a PDF can become a Word file");
        assert_eq!(docx.available, PDFCRAFT.find().is_some());
    }

    #[test]
    fn several_types_keep_only_what_all_of_them_can_become() {
        let both = common(&["png".to_owned(), "jpg".to_owned()]);
        assert!(both.iter().any(|s| s.id == "webp"), "{both:?}");
        // each type is a format the other one could become, but not a format a file already has
        assert!(both.iter().all(|s| s.id != "png" && s.id != "jpg"), "{both:?}");
        // a picture and a PDF share exactly one output: JPG (the PDF's pages as JPG pictures, the picture as a JPG)
        let mixed = common(&["png".to_owned(), "pdf".to_owned()]);
        assert_eq!(ids(&mixed), ["jpg"], "{mixed:?}");
        // a picture and a sound file share nothing
        let none = common(&["png".to_owned(), "mp3".to_owned()]);
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn unknown_types_offer_nothing() {
        assert!(offer(&["xyz".to_owned()]).is_empty());
        assert!(offer(&[]).is_empty());
    }

    #[test]
    fn every_offered_slot_runs_a_real_route() {
        for ext in ["pdf", "docx", "xlsx", "csv", "png", "jpg", "mp3", "mp4", "epub", "eml", "numbers", "heic"] {
            let o = offer(&[ext.to_owned()]);
            for s in o.slots.iter().chain(&o.more).filter(|s| !s.id.is_empty()) {
                assert!(routes::find_route(ext, &s.id).is_some(), "{ext} -> {}", s.id);
            }
        }
    }
}
