//! Apple iWork files (`.numbers` for now) read without Apple's software.
//!
//! An iWork '13+ document is a zip with `Index/*.iwa` files. An IWA file is a run of chunks (`0x00`, a 3-byte
//! little-endian length, one raw Snappy block, no checksum); the decompressed chunks together are a sequence of
//! archives, each a varint-length `ArchiveInfo` header followed by the Protocol Buffers messages it describes.
//! There is no `protoc` here and no schema crate: [`Wire`] reads the few fields a table needs by number.
//!
//! The layout (message type ids, field numbers, the cell storage format) follows the open reverse-engineering work
//! of `numbers-parser` by Jon Connell (MIT), `iWorkFileFormat` by Peter O'Brien (MIT) and `keynote-parser` by Peter
//! Sobot (MIT); see CREDITS.
//!
//! Nothing is rounded on the way: a Numbers number is a decimal128, written here as the exact decimal it holds.
//! What this reader cannot read is counted in [`Opened::notes`], never silently turned into a value.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// One table of a Numbers document, as text cells in row order, trailing empty rows and columns dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub sheet: String,
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct Opened {
    pub tables: Vec<Table>,
    /// Things that were not read exactly (cells of a kind or version this reader does not know).
    pub notes: Vec<String>,
}

const MAX_OBJECT_BYTES: usize = 256 << 20;
const DEFAULT_TILE_SIZE: u64 = 256;

// ---- Protocol Buffers wire format -------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(crate) enum Val<'a> {
    Varint(u64),
    Fixed(u64),
    Bytes(&'a [u8]),
}

/// The fields of one message in wire order. Stops at the first thing that does not parse.
pub(crate) struct Wire<'a> {
    buf: &'a [u8],
    pos: usize,
}

pub(crate) fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7F).checked_shl(shift).unwrap_or(0);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

impl<'a> Iterator for Wire<'a> {
    type Item = (u32, Val<'a>);
    fn next(&mut self) -> Option<Self::Item> {
        let key = varint(self.buf, &mut self.pos)?;
        let (no, kind) = ((key >> 3) as u32, key & 7);
        let val = match kind {
            0 => Val::Varint(varint(self.buf, &mut self.pos)?),
            1 => {
                let b = self.buf.get(self.pos..self.pos.checked_add(8)?)?;
                self.pos += 8;
                Val::Fixed(u64::from_le_bytes(b.try_into().ok()?))
            }
            2 => {
                let len = usize::try_from(varint(self.buf, &mut self.pos)?).ok()?;
                let b = self.buf.get(self.pos..self.pos.checked_add(len)?)?;
                self.pos += len;
                Val::Bytes(b)
            }
            5 => {
                let b = self.buf.get(self.pos..self.pos.checked_add(4)?)?;
                self.pos += 4;
                Val::Fixed(u64::from(u32::from_le_bytes(b.try_into().ok()?)))
            }
            _ => return None,
        };
        Some((no, val))
    }
}

pub(crate) fn wire(buf: &[u8]) -> Wire<'_> {
    Wire { buf, pos: 0 }
}

pub(crate) fn bytes_of(buf: &[u8], no: u32) -> Option<&[u8]> {
    wire(buf).find_map(|(n, v)| match v {
        Val::Bytes(b) if n == no => Some(b),
        _ => None,
    })
}

pub(crate) fn all_bytes_of(buf: &[u8], no: u32) -> Vec<&[u8]> {
    wire(buf)
        .filter_map(|(n, v)| match v {
            Val::Bytes(b) if n == no => Some(b),
            _ => None,
        })
        .collect()
}

pub(crate) fn uint_of(buf: &[u8], no: u32) -> Option<u64> {
    wire(buf).find_map(|(n, v)| match v {
        Val::Varint(x) | Val::Fixed(x) if n == no => Some(x),
        _ => None,
    })
}

pub(crate) fn text_of(buf: &[u8], no: u32) -> Option<String> {
    bytes_of(buf, no).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// A `TSP.Reference` field: the id of the object it points to.
pub(crate) fn ref_of(buf: &[u8], no: u32) -> Option<u64> {
    bytes_of(buf, no).and_then(|r| uint_of(r, 1))
}

pub(crate) fn refs_of(buf: &[u8], no: u32) -> Vec<u64> {
    all_bytes_of(buf, no).into_iter().filter_map(|r| uint_of(r, 1)).collect()
}

// ---- Snappy ------------------------------------------------------------------------------------------------

/// One raw Snappy block (no stream framing). Every index is checked: the file comes from outside.
pub fn snappy_block(input: &[u8]) -> Result<Vec<u8>, String> {
    let bad = || "damaged compressed data".to_owned();
    let mut pos = 0;
    let want = usize::try_from(varint(input, &mut pos).ok_or_else(bad)?).map_err(|_| bad())?;
    if want > MAX_OBJECT_BYTES {
        return Err("a compressed block is too large".to_owned());
    }
    let mut out: Vec<u8> = Vec::with_capacity(want);
    while pos < input.len() {
        let tag = input[pos];
        pos += 1;
        match tag & 3 {
            0 => {
                let mut len = usize::from(tag >> 2);
                if len >= 60 {
                    let n = len - 59;
                    let b = input.get(pos..pos + n).ok_or_else(bad)?;
                    len = b.iter().rev().fold(0usize, |a, &x| (a << 8) | usize::from(x));
                    pos += n;
                }
                len += 1;
                out.extend_from_slice(input.get(pos..pos.checked_add(len).ok_or_else(bad)?).ok_or_else(bad)?);
                pos += len;
            }
            kind => {
                let (len, off) = match kind {
                    1 => {
                        let o = (usize::from(tag >> 5) << 8) | usize::from(*input.get(pos).ok_or_else(bad)?);
                        pos += 1;
                        (usize::from((tag >> 2) & 7) + 4, o)
                    }
                    2 => {
                        let b = input.get(pos..pos + 2).ok_or_else(bad)?;
                        pos += 2;
                        (usize::from(tag >> 2) + 1, usize::from(u16::from_le_bytes([b[0], b[1]])))
                    }
                    _ => {
                        let b = input.get(pos..pos + 4).ok_or_else(bad)?;
                        pos += 4;
                        (usize::from(tag >> 2) + 1, u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                    }
                };
                if off == 0 || off > out.len() || out.len() + len > MAX_OBJECT_BYTES {
                    return Err(bad());
                }
                // may overlap itself: byte by byte
                for _ in 0..len {
                    out.push(out[out.len() - off]);
                }
            }
        }
    }
    if out.len() != want {
        return Err(bad());
    }
    Ok(out)
}

/// An IWA file's chunks, decompressed and joined.
pub fn iwa_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0 {
            return Err("not an IWA chunk".to_owned());
        }
        let head = data.get(i + 1..i + 4).ok_or("cut-off IWA chunk")?;
        let len = usize::from(head[0]) | (usize::from(head[1]) << 8) | (usize::from(head[2]) << 16);
        let block = data.get(i + 4..i + 4 + len).ok_or("cut-off IWA chunk")?;
        out.extend(snappy_block(block)?);
        if out.len() > MAX_OBJECT_BYTES {
            return Err("an IWA file is too large".to_owned());
        }
        i += 4 + len;
    }
    Ok(out)
}

// ---- The object store --------------------------------------------------------------------------------------

/// Every archive of a document by its id: the type of its first message and that message's bytes.
pub(crate) struct Store {
    objects: HashMap<u64, (u32, Vec<u8>)>,
}

impl Store {
    fn add_iwa(&mut self, raw: &[u8]) -> Result<(), String> {
        let buf = iwa_bytes(raw)?;
        let mut pos = 0;
        while pos < buf.len() {
            let hlen = usize::try_from(varint(&buf, &mut pos).ok_or("damaged archive header")?).map_err(|_| "damaged archive header")?;
            let header = buf.get(pos..pos.checked_add(hlen).ok_or("damaged archive header")?).ok_or("damaged archive header")?;
            pos += hlen;
            let id = uint_of(header, 1).ok_or("archive without an id")?;
            let mut first = true;
            for info in all_bytes_of(header, 2) {
                let ty = uint_of(info, 1).unwrap_or(0) as u32;
                let len = uint_of(info, 3).unwrap_or(0) as usize;
                let body = buf.get(pos..pos.checked_add(len).ok_or("damaged archive")?).ok_or("damaged archive")?;
                pos += len;
                if first {
                    self.objects.insert(id, (ty, body.to_vec()));
                    first = false;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn get(&self, id: u64) -> Option<&(u32, Vec<u8>)> {
        self.objects.get(&id)
    }

    /// Every object as (id, message type, bytes), in no particular order.
    pub(crate) fn all(&self) -> impl Iterator<Item = (u64, u32, &[u8])> {
        self.objects.iter().map(|(id, (ty, body))| (*id, *ty, body.as_slice()))
    }
}

fn read_zip_entry<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, i: usize) -> Result<Vec<u8>, String> {
    let f = z.by_index(i).map_err(|e| e.to_string())?;
    if f.size() > MAX_OBJECT_BYTES as u64 {
        return Err("a file inside the document is too large".to_owned());
    }
    let mut v = Vec::with_capacity(f.size() as usize);
    f.take(MAX_OBJECT_BYTES as u64).read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(v)
}

fn load<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, store: &mut Store, depth: u8) -> Result<(), String> {
    for i in 0..z.len() {
        let name = {
            let f = z.by_index(i).map_err(|e| e.to_string())?;
            f.name().map_err(|e| e.to_string())?.into_owned()
        };
        if name.ends_with(".iwa") && name.contains("Index/") {
            store.add_iwa(&read_zip_entry(z, i)?).map_err(|e| format!("{name}: {e}"))?;
        } else if depth == 0 && name == "Index.zip" {
            // older '13 documents keep the Index folder in a zip of its own
            let inner = read_zip_entry(z, i)?;
            let mut inner = zip::ZipArchive::new(std::io::Cursor::new(inner)).map_err(|e| e.to_string())?;
            load(&mut inner, store, 1)?;
        }
    }
    Ok(())
}

// ---- Cells -------------------------------------------------------------------------------------------------

/// A decimal128 as the exact decimal it holds: `1770.30` is `1770.3`, never `1770.2999999999999`.
pub fn decimal128(b: &[u8]) -> Option<String> {
    let b: &[u8; 16] = b.get(..16)?.try_into().ok()?;
    let exp = ((i64::from(b[15] & 0x7F) << 7) | i64::from(b[14] >> 1)) - 0x1820;
    let mut mantissa = u128::from(b[14] & 1);
    for i in (0..14).rev() {
        mantissa = (mantissa << 8) | u128::from(b[i]);
    }
    let negative = b[15] & 0x80 != 0 && mantissa != 0;
    if mantissa == 0 {
        return Some("0".to_owned());
    }
    let digits = mantissa.to_string();
    let mut s = if exp >= 0 {
        if exp > 60 {
            return None;
        }
        format!("{digits}{}", "0".repeat(exp as usize))
    } else {
        let places = (-exp) as usize;
        if places > 80 {
            return None;
        }
        let padded = if digits.len() <= places { format!("{}{digits}", "0".repeat(places - digits.len() + 1)) } else { digits };
        let (int, frac) = padded.split_at(padded.len() - places);
        format!("{int}.{frac}")
    };
    if s.contains('.') {
        s.truncate(s.trim_end_matches('0').trim_end_matches('.').len());
    }
    Some(if negative { format!("-{s}") } else { s })
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Numbers counts seconds from 2001-01-01. `2026-11-05` when it is midnight, else `2026-11-05 14:30:00`.
pub fn date_text(seconds: f64) -> String {
    if !seconds.is_finite() {
        return String::new();
    }
    let whole = seconds.round() as i64;
    let (days, secs) = (whole.div_euclid(86_400), whole.rem_euclid(86_400));
    let (y, m, d) = civil(days + 11_323); // 2001-01-01 is day 11323 of the Unix epoch
    if secs == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    }
}

pub fn duration_text(seconds: f64) -> String {
    if !seconds.is_finite() {
        return String::new();
    }
    let total = seconds.abs().round() as u64;
    let sign = if seconds < 0.0 && total > 0 { "-" } else { "" };
    format!("{sign}{}:{:02}:{:02}", total / 3600, total % 3600 / 60, total % 60)
}

pub(crate) struct Strings {
    pub(crate) by_key: HashMap<u64, String>,
}

/// The entries of a `TableDataList` and of its segments.
pub(crate) fn list_entries<'a>(store: &'a Store, id: u64, out: &mut Vec<&'a [u8]>, depth: u8) {
    let Some((_, body)) = store.get(id) else { return };
    out.extend(all_bytes_of(body, 3));
    if depth < 3 {
        for seg in refs_of(body, 4) {
            list_entries(store, seg, out, depth + 1);
        }
    }
}

pub(crate) fn string_table(store: &Store, id: u64) -> Strings {
    let mut entries = Vec::new();
    list_entries(store, id, &mut entries, 0);
    Strings { by_key: entries.into_iter().filter_map(|e| Some((uint_of(e, 1)?, text_of(e, 3)?))).collect() }
}

/// The text of a rich text cell: its payload's storage holds the paragraphs.
pub(crate) fn rich_text(store: &Store, list: u64, key: u64) -> Option<String> {
    let mut entries = Vec::new();
    list_entries(store, list, &mut entries, 0);
    let entry = entries.into_iter().find(|e| uint_of(e, 1) == Some(key))?;
    let payload = store.get(ref_of(entry, 9)?)?;
    let storage = store.get(ref_of(&payload.1, 1)?)?;
    text_of(&storage.1, 3)
}

struct TableCtx<'a> {
    store: &'a Store,
    strings: Strings,
    rich_list: u64,
}

const CELL_EMPTY: u8 = 0;
const CELL_NUMBER: u8 = 2;
const CELL_TEXT: u8 = 3;
const CELL_DATE: u8 = 5;
const CELL_BOOL: u8 = 6;
const CELL_DURATION: u8 = 7;
const CELL_ERROR: u8 = 8;
const CELL_RICH: u8 = 9;
const CELL_CURRENCY: u8 = 10;

#[derive(Default)]
struct Unread {
    versions: usize,
    kinds: usize,
    errors: usize,
}

fn cell_text(buf: &[u8], ctx: &TableCtx, unread: &mut Unread) -> String {
    if buf.len() < 12 {
        return String::new();
    }
    if buf[0] != 5 {
        unread.versions += 1;
        return String::new();
    }
    let flags = i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let mut off = 12usize;
    let mut take = |n: usize| {
        let s = buf.get(off..off + n);
        off += n;
        s
    };
    let d128 = if flags & 0x1 != 0 { take(16).and_then(decimal128) } else { None };
    let double = if flags & 0x2 != 0 { take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8]))) } else { None };
    let seconds = if flags & 0x4 != 0 { take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8]))) } else { None };
    let mut int = |present: bool| if present { take(4).map(|b| u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))) } else { None };
    let string_id = int(flags & 0x8 != 0);
    let rich_id = int(flags & 0x10 != 0);
    match buf[1] {
        CELL_EMPTY => String::new(),
        CELL_NUMBER | CELL_CURRENCY => d128.unwrap_or_else(|| "0".to_owned()),
        CELL_TEXT => ctx.strings.by_key.get(&string_id.unwrap_or(0)).cloned().unwrap_or_default(),
        CELL_DATE => date_text(seconds.unwrap_or(0.0)),
        CELL_BOOL => if double.unwrap_or(0.0) > 0.0 { "TRUE" } else { "FALSE" }.to_owned(),
        CELL_DURATION => duration_text(double.unwrap_or(0.0)),
        CELL_ERROR => {
            unread.errors += 1;
            "#ERROR".to_owned()
        }
        CELL_RICH => rich_id.and_then(|k| rich_text(ctx.store, ctx.rich_list, k)).unwrap_or_default(),
        _ => {
            unread.kinds += 1;
            String::new()
        }
    }
}

/// The cells of one tile row: `offsets` are signed 16-bit (a negative one means no cell), in units of 4 bytes when `wide`.
pub(crate) fn row_cells<'a>(buffer: &'a [u8], offsets: &[u8], wide: bool, columns: usize) -> Vec<Option<&'a [u8]>> {
    let offs: Vec<i32> = offsets.chunks_exact(2).map(|b| i32::from(i16::from_le_bytes([b[0], b[1]])) * if wide { 4 } else { 1 }).collect();
    (0..columns.min(offs.len()))
        .map(|c| {
            let start = usize::try_from(offs[c]).ok()?;
            let end = offs[c + 1..].iter().find_map(|&o| usize::try_from(o).ok()).unwrap_or(buffer.len());
            buffer.get(start..end.max(start).min(buffer.len()))
        })
        .collect()
}

// ---- The document ------------------------------------------------------------------------------------------

const TYPE_DOCUMENT: u64 = 1;
const TYPE_TABLE_INFO: u32 = 6000;

fn read_table(store: &Store, model_id: u64, sheet: &str, unread: &mut Unread) -> Option<Table> {
    let (_, model) = store.get(model_id)?;
    let name = text_of(model, 8).unwrap_or_default();
    let rows_n = uint_of(model, 6)? as usize;
    let cols_n = uint_of(model, 7)? as usize;
    if rows_n.checked_mul(cols_n)? > 50_000_000 {
        return None;
    }
    let data = bytes_of(model, 4)?;
    let tiles = bytes_of(data, 3)?;
    let tile_size = uint_of(tiles, 2).filter(|&s| s > 0).unwrap_or(DEFAULT_TILE_SIZE) as usize;
    let ctx = TableCtx { store, strings: ref_of(data, 4).map(|id| string_table(store, id)).unwrap_or(Strings { by_key: HashMap::new() }), rich_list: ref_of(data, 17).unwrap_or(0) };

    let mut grid = vec![vec![String::new(); cols_n]; rows_n];
    for t in all_bytes_of(tiles, 1) {
        let (Some(tile_no), Some(tile_ref)) = (uint_of(t, 1), ref_of(t, 2)) else { continue };
        let Some((_, tile)) = store.get(tile_ref) else { continue };
        for info in all_bytes_of(tile, 5) {
            let row = (tile_no as usize).saturating_mul(tile_size).saturating_add(uint_of(info, 1).unwrap_or(0) as usize);
            if row >= rows_n {
                continue;
            }
            let (Some(buffer), Some(offsets)) = (bytes_of(info, 6), bytes_of(info, 7)) else { continue };
            let wide = uint_of(info, 8).unwrap_or(0) != 0;
            for (c, cell) in row_cells(buffer, offsets, wide, cols_n).into_iter().enumerate() {
                if let Some(cell) = cell {
                    grid[row][c] = cell_text(cell, &ctx, unread);
                }
            }
        }
    }
    // Empty rows and columns at the end carry nothing (a Numbers table keeps its drawn size, not its data).
    while grid.last().is_some_and(|r| r.iter().all(String::is_empty)) {
        grid.pop();
    }
    let width = grid.iter().map(|r| r.iter().rposition(|c| !c.is_empty()).map_or(0, |p| p + 1)).max().unwrap_or(0);
    for r in &mut grid {
        r.truncate(width);
    }
    Some(Table { sheet: sheet.to_owned(), name, rows: grid })
}

/// Opens an iWork document (a zip of IWA files) and decodes every archive in it. `kind` names the app in the messages
/// ("Numbers", "Pages", "Keynote"). An empty file is what iCloud Drive leaves for a document it has not downloaded.
pub(crate) fn open_store(path: &Path, kind: &str) -> Result<Store, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() == 0 {
        return Err("The file is empty. If it lives in iCloud Drive it has not been downloaded to this PC yet: right-click it and choose \"Always keep on this device\".".to_owned());
    }
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| format!("This is not a {kind} file (or it is a package folder: zip it first)."))?;
    let mut store = Store { objects: HashMap::new() };
    load(&mut z, &mut store, 0)?;
    if store.get(TYPE_DOCUMENT).is_none() {
        return Err(format!("This {kind} file has no document part (is it from an old version of {kind}, before 2013?)."));
    }
    Ok(store)
}

/// Reads every table of a `.numbers` file.
pub fn read_numbers(path: &Path) -> Result<Opened, String> {
    let store = open_store(path, "Numbers")?;
    let Some((_, doc)) = store.get(TYPE_DOCUMENT) else { return Err("Unexpected document structure.".to_owned()) };
    let mut opened = Opened::default();
    let mut unread = Unread::default();
    for sheet_id in refs_of(doc, 1) {
        let Some((_, sheet)) = store.get(sheet_id) else { continue };
        let sheet_name = text_of(sheet, 1).unwrap_or_default();
        for drawable in refs_of(sheet, 2) {
            let Some((dty, info)) = store.get(drawable) else { continue };
            if *dty != TYPE_TABLE_INFO {
                continue;
            }
            if let Some(model) = ref_of(info, 2)
                && let Some(table) = read_table(&store, model, &sheet_name, &mut unread)
            {
                opened.tables.push(table);
            }
        }
    }
    if unread.versions > 0 {
        opened.notes.push(format!("{} cells use a storage version this reader does not know and were left empty.", unread.versions));
    }
    if unread.kinds > 0 {
        opened.notes.push(format!("{} cells are of a kind this reader does not know and were left empty.", unread.kinds));
    }
    if unread.errors > 0 {
        opened.notes.push(format!("{} cells hold a formula error in Numbers; they are written as #ERROR.", unread.errors));
    }
    if opened.tables.is_empty() {
        return Err("No table found in this Numbers file.".to_owned());
    }
    Ok(opened)
}

/// A table as CSV: UTF-8, commas, CRLF, quotes only where needed.
pub fn csv(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .map(|c| if c.contains([',', '"', '\n', '\r']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
            .collect();
        out += &line.join(",");
        out += "\r\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snappy_literals_and_overlapping_copies() {
        // length 11: literal "ab", then copy 9 bytes from offset 2 (overlapping): "abababababa"
        let block = [11u8, 0x04, b'a', b'b', ((9 - 4) << 2) | 1, 2];
        assert_eq!(snappy_block(&block).unwrap(), b"abababababa");
        assert!(snappy_block(&[5, 0x0C, b'a']).is_err(), "a literal that runs past the end");
        assert!(snappy_block(&[4, 0x01, 9]).is_err(), "a copy from before the start");
        assert!(snappy_block(&[3, 0x00, b'a']).is_err(), "length that does not match");
    }

    #[test]
    fn iwa_chunks_join() {
        // a Snappy block of length 1 holding the literal "x", twice, each in its own chunk
        let mut data = Vec::new();
        let one = [1u8, 0x00, b'x'];
        for _ in 0..2 {
            data.extend([0, one.len() as u8, 0, 0]);
            data.extend(one);
        }
        assert_eq!(iwa_bytes(&data).unwrap(), b"xx");
        assert!(iwa_bytes(&[1, 2, 3]).is_err());
        assert!(iwa_bytes(&[0, 9, 0, 0, 1]).is_err(), "chunk longer than the data");
    }

    #[test]
    fn decimals_are_exact() {
        let mk = |mantissa: u128, exp: i64, neg: bool| {
            let mut b = [0u8; 16];
            let biased = (exp + 0x1820) as u64;
            for i in 0..14 {
                b[i] = (mantissa >> (8 * i)) as u8;
            }
            b[14] = (((biased & 0x7F) << 1) as u8) | ((mantissa >> 112) as u8 & 1);
            b[15] = ((biased >> 7) as u8) | if neg { 0x80 } else { 0 };
            b
        };
        assert_eq!(decimal128(&mk(177030, -2, false)).unwrap(), "1770.3");
        assert_eq!(decimal128(&mk(5, -3, true)).unwrap(), "-0.005");
        assert_eq!(decimal128(&mk(12, 2, false)).unwrap(), "1200");
        assert_eq!(decimal128(&mk(0, -2, true)).unwrap(), "0");
        assert_eq!(decimal128(&mk(1234567890123456789012345, -10, false)).unwrap(), "123456789012345.6789012345");
    }

    #[test]
    fn dates_count_from_2001() {
        assert_eq!(date_text(0.0), "2001-01-01");
        assert_eq!(date_text(86_400.0 * 365.0), "2002-01-01");
        assert_eq!(date_text(-86_400.0), "2000-12-31");
        // 25 years with 6 leap days reach 2026-01-01 (9131 days); 304 days of Jan-Oct and 4 more make 2026-11-05
        assert_eq!(date_text(86_400.0 * 9_439.0), "2026-11-05");
        assert_eq!(date_text(86_400.0 * 9_439.0 + 3_661.0), "2026-11-05 01:01:01");
        // a leap day
        assert_eq!(date_text(86_400.0 * 1_154.0), "2004-02-29");
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(3_661.0), "1:01:01");
        assert_eq!(duration_text(-90.0), "-0:01:30");
    }

    #[test]
    fn tile_rows_split_at_the_next_offset() {
        let buffer = b"AAAABBBBCC";
        let offsets: Vec<u8> = [0i16, -1, 4, 8].iter().flat_map(|o| o.to_le_bytes()).collect();
        let cells = row_cells(buffer, &offsets, false, 4);
        assert_eq!(cells[0], Some(&b"AAAA"[..]));
        assert_eq!(cells[1], None);
        assert_eq!(cells[2], Some(&b"BBBB"[..]));
        assert_eq!(cells[3], Some(&b"CC"[..]));
        let wide: Vec<u8> = [0i16, 1].iter().flat_map(|o| o.to_le_bytes()).collect();
        assert_eq!(row_cells(buffer, &wide, true, 2)[1], Some(&b"BBBBCC"[..]));
    }

    #[test]
    fn csv_quotes_only_when_needed() {
        let rows = vec![vec!["a".into(), "b,c".into(), "say \"hi\"".into()], vec!["x\ny".into(), String::new()]];
        assert_eq!(csv(&rows), "a,\"b,c\",\"say \"\"hi\"\"\"\r\n\"x\ny\",\r\n");
    }

    #[test]
    fn a_zero_byte_or_foreign_file_says_what_is_wrong() {
        let dir = std::env::temp_dir().join(format!("vf_iwork_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("a.numbers");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_numbers(&empty).unwrap_err().contains("iCloud"));
        let junk = dir.join("b.numbers");
        std::fs::write(&junk, b"not a zip").unwrap();
        assert!(read_numbers(&junk).unwrap_err().contains("not a Numbers"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With `VOIDFLARE_TEST_NUMBERS` set to a real `.numbers` file: it reads, and the tables print (`--nocapture`).
    #[test]
    fn a_real_numbers_file_reads() {
        let Some(path) = std::env::var_os("VOIDFLARE_TEST_NUMBERS") else { return };
        let opened = read_numbers(Path::new(&path)).expect("reads");
        assert!(!opened.tables.is_empty());
        for t in &opened.tables {
            println!("== {} / {} ({} rows)", t.sheet, t.name, t.rows.len());
            print!("{}", csv(&t.rows));
        }
        for n in &opened.notes {
            println!("NOTE: {n}");
        }
    }
}
