//! An Apple Numbers document as a workbook: values, number formats, fonts and fills, formulas, merged cells, column
//! widths and frozen headers, read without Apple's software (see [`crate::iwork`] for the container).
//!
//! What a cell shows is not what it holds: `1770.3` with a Euro format is "€ 1.770,30"; a negative in red with no minus
//! sign is still a negative. So the value is kept exact and the look is carried next to it as an Excel number format
//! code, a font and a fill, and the workbook is written by GridCraft (see [`script`]) with both.
//!
//! The layout (message type ids, field numbers, the cell storage, the formula node stack) follows `numbers-parser` by
//! Jon Connell and `iWorkFileFormat` by Peter O'Brien, both MIT; see CREDITS.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::iwork::{self, Store, Strings, all_bytes_of, bytes_of, ref_of, refs_of, text_of, uint_of, wire};
use crate::numbers_functions;

const TYPE_TABLE_INFO: u32 = 6000;
const TYPE_CHART_DRAWABLE: u32 = 5021;
const TYPE_FORMULA_OWNER: u32 = 4008;
const OWNER_KIND_HAUNTED: u64 = 35;
const DECIMAL_PLACES_AUTO: u64 = 253;
const NUMBER_NODE_DECIMAL_HIGH: u64 = 0x3040_0000_0000_0000;
/// Excel's serial number of 2001-01-01, the day Numbers counts from.
const SERIAL_2001: f64 = 36_892.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    /// The exact decimal the cell holds.
    Number(String),
    Text(String),
    /// Seconds since 2001-01-01.
    Date(f64),
    Bool(bool),
    /// Seconds.
    Duration(f64),
    Error,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub size: Option<f32>,
    pub font: Option<String>,
    pub color: Option<[u8; 3]>,
    pub fill: Option<[u8; 3]>,
    /// 0 left, 1 right, 2 centre, 3 justified (None: automatic, as General in a spreadsheet).
    pub halign: Option<u8>,
    /// 0 top, 1 middle, 2 bottom.
    pub valign: Option<u8>,
    pub wrap: bool,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub value: Value,
    /// The formula in Excel syntax (`=SUM(B2:B3)`), when Numbers kept one this reader could translate.
    pub formula: Option<String>,
    /// An Excel number format code; None is General.
    pub numfmt: Option<String>,
    pub style: Style,
    /// The key of the conditional style set the cell has (see [`Table::cond_sets`]).
    pub cond: Option<u32>,
}

/// "When the cell's value is `op` `value`, show it in `style`": Numbers' conditional styles, as Excel's `cellIs` rules.
#[derive(Debug, Clone, PartialEq)]
pub struct CondRule {
    /// lessThan, lessThanOrEqual, greaterThan, greaterThanOrEqual, equal, notEqual
    pub op: &'static str,
    pub value: f64,
    pub style: Style,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub sheet: String,
    pub name: String,
    /// The worksheet's name in the workbook (unique, 31 characters at most).
    pub xl_name: String,
    pub cells: Vec<Vec<Cell>>,
    /// First row, first column, last row, last column.
    pub merges: Vec<[usize; 4]>,
    /// In points, one per column.
    pub col_widths: Vec<f32>,
    /// In points, one per row (the default when the row has no height of its own).
    pub row_heights: Vec<f32>,
    pub header_rows: usize,
    pub header_cols: usize,
    pub hidden_cols: Vec<usize>,
    pub hidden_rows: Vec<usize>,
    /// Conditional style sets by key: the rules of each.
    pub cond_sets: HashMap<u32, Vec<CondRule>>,
    /// Where the table sits on its sheet's canvas, in points.
    pub origin: (f32, f32),
    pub charts: Vec<Chart>,
}

/// A chart: its type, its data (Numbers keeps a copy in the chart itself) and where it sits next to its table.
#[derive(Debug, Clone, Default)]
pub struct Chart {
    /// GridCraft's names: column, bar, line, pie, doughnut, area, scatter, bubble, radar.
    pub kind: &'static str,
    /// "" (clustered) or stacked.
    pub subtype: &'static str,
    pub categories: Vec<String>,
    /// One series per entry: its name and a value for each category.
    pub series: Vec<(String, Vec<Option<f64>>)>,
    /// Position and size in points, relative to the table's top left corner.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Default)]
pub struct Workbook {
    pub tables: Vec<Table>,
    pub notes: Vec<String>,
}

/// A date (seconds since 2001-01-01) as an Excel serial number.
pub fn excel_serial(seconds: f64) -> f64 {
    SERIAL_2001 + seconds / 86_400.0
}

// ---- small readers -------------------------------------------------------------------------------------------

fn bits_of(buf: &[u8], no: u32) -> Option<u64> {
    wire(buf).find_map(|(n, v)| match v {
        iwork::Val::Fixed(b) if n == no => Some(b),
        _ => None,
    })
}

fn f64_of(buf: &[u8], no: u32) -> Option<f64> {
    bits_of(buf, no).map(f64::from_bits)
}

fn f32_of(buf: &[u8], no: u32) -> Option<f32> {
    bits_of(buf, no).map(|b| f32::from_bits(b as u32))
}

fn flag_of(buf: &[u8], no: u32) -> Option<bool> {
    uint_of(buf, no).map(|v| v != 0)
}

fn zigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// A `TSP.UUID` message as one number.
fn uuid_of(msg: &[u8]) -> Option<u128> {
    Some((u128::from(uint_of(msg, 2)?) << 64) | u128::from(uint_of(msg, 1)?))
}

/// A `TSP.CFUUIDArchive` message as the same number.
fn cfuuid_of(msg: &[u8]) -> Option<u128> {
    let w = |n| uint_of(msg, n).map(u128::from);
    Some((w(5)? << 96) | (w(4)? << 64) | (w(3)? << 32) | w(2)?)
}

fn rgb_of(color: &[u8]) -> Option<[u8; 3]> {
    if uint_of(color, 1)? != 1 {
        return None; // CMYK and grey are rare in cell styles
    }
    let ch = |n| f32_of(color, n).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some([ch(3)?, ch(4)?, ch(5)?])
}

fn column_letters(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

pub fn cell_name(row: usize, col: usize) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

/// A message as a tree of `field=value`, for working out what an unfamiliar one holds (debug only).
fn dump_msg(buf: &[u8], depth: usize) -> String {
    let mut parts = Vec::new();
    for (n, v) in wire(buf) {
        parts.push(match v {
            iwork::Val::Varint(x) => format!("{n}={x}"),
            iwork::Val::Fixed(x) => format!("{n}=f{x:#x}"),
            iwork::Val::Bytes(b) => {
                if depth > 0 && !b.is_empty() && wire(b).next().is_some() && wire(b).map(|(_, v)| match v { iwork::Val::Bytes(x) => x.len(), _ => 0 }).sum::<usize>() <= b.len() && b.iter().all(|&c| c != 0xFF) && std::str::from_utf8(b).map_or(true, |s| s.chars().any(char::is_control)) {
                    format!("{n}={{{}}}", dump_msg(b, depth - 1))
                } else if let Ok(s) = std::str::from_utf8(b) {
                    format!("{n}={s:?}")
                } else {
                    format!("{n}=<{} bytes>", b.len())
                }
            }
        });
    }
    parts.join(" ")
}

fn list_by_key<'a>(store: &'a Store, id: Option<u64>) -> HashMap<u64, &'a [u8]> {
    let mut out = Vec::new();
    if let Some(id) = id {
        iwork::list_entries(store, id, &mut out, 0);
    }
    out.into_iter().filter_map(|e| Some((uint_of(e, 1)?, e))).collect()
}

// ---- number formats ------------------------------------------------------------------------------------------

fn currency_symbol(code: &str) -> String {
    match code {
        "EUR" => "€".into(),
        "USD" => "$".into(),
        "GBP" => "£".into(),
        "JPY" => "¥".into(),
        "CNY" => "CN¥".into(),
        "CHF" => "CHF".into(),
        "AUD" => "A$".into(),
        "CAD" => "CA$".into(),
        "BRL" => "R$".into(),
        "INR" => "₹".into(),
        "KRW" => "₩".into(),
        "RUB" => "₽".into(),
        "" => String::new(),
        other => other.to_owned(),
    }
}

/// `"text"` for an Excel format code: quotes inside it are dropped (a format cannot hold them).
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

/// Numbers' date pattern (`d MMM yyyy`, `HH:mm`) as an Excel one (`d mmm yyyy`, `hh:mm`).
fn date_pattern(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            let mut lit = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                lit.push(chars[i]);
                i += 1;
            }
            i += 1;
            if lit.is_empty() {
                out.push('\'');
            } else {
                out += &quoted(&lit);
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            match c {
                '/' | '-' | ':' | ',' | '.' | ' ' | '(' | ')' => out.push(c),
                _ => {
                    out.push('\\');
                    out.push(c);
                }
            }
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        i += n;
        match c {
            'y' | 'Y' | 'u' => out += if n >= 3 { "yyyy" } else { "yy" },
            'M' | 'L' => out += ["m", "mm", "mmm", "mmmm", "mmmm"][(n - 1).min(4)],
            'd' => out += if n >= 2 { "dd" } else { "d" },
            'E' | 'e' | 'c' => out += if n >= 4 { "dddd" } else { "ddd" },
            'a' | 'b' | 'B' => out += "AM/PM",
            'H' | 'h' | 'k' | 'K' => out += if n >= 2 { "hh" } else { "h" },
            'm' => out += if n >= 2 { "mm" } else { "m" },
            's' => out += if n >= 2 { "ss" } else { "s" },
            'S' => {
                out.push('.');
                out += &"0".repeat(n);
            }
            _ => {} // time zones, eras, week numbers: no Excel equivalent
        }
    }
    out
}

/// The Excel number format code for a `TSK.FormatStructArchive`, or None for General. `custom` maps a custom format's
/// uuid to its `CustomFormatArchive`.
fn excel_format(f: &[u8], custom: &HashMap<u128, &[u8]>, notes: &mut HashSet<String>) -> Option<String> {
    if let Some(uid) = bytes_of(f, 41).and_then(uuid_of) {
        let whole = custom.get(&uid)?;
        let def = bytes_of(whole, 3)?;
        let conditions: Vec<(u64, f64, String)> = all_bytes_of(whole, 4)
            .into_iter()
            .filter_map(|c| {
                let value = f64_of(c, 4).or_else(|| f32_of(c, 2).map(f64::from)).unwrap_or(0.0);
                Some((uint_of(c, 1)?, value, custom_format(bytes_of(c, 3)?, notes)?))
            })
            .collect();
        return custom_sections(custom_format(def, notes)?, &conditions, notes);
    }
    let ty = uint_of(f, 1)?;
    let places = uint_of(f, 2).unwrap_or(DECIMAL_PLACES_AUTO);
    let thousands = flag_of(f, 5).unwrap_or(false);
    let neg = uint_of(f, 4).unwrap_or(0);
    let digits = |thousands: bool| -> String {
        let base = if thousands { "#,##0" } else { "0" };
        match places {
            DECIMAL_PLACES_AUTO => if thousands { "#,##0.##".into() } else { "General".into() },
            0 => base.into(),
            n => format!("{base}.{}", "0".repeat(n.min(30) as usize)),
        }
    };
    let with_negative = |pos: String| -> String {
        match neg {
            1 => format!("{pos};[Red]{pos}"),
            2 => format!("{pos}_);({pos})"),
            3 => format!("{pos}_);[Red]({pos})"),
            _ => pos,
        }
    };
    match ty {
        256 => {
            let d = digits(thousands);
            if d == "General" && neg == 0 { None } else { Some(with_negative(d)) }
        }
        257 => {
            let sym = currency_symbol(&text_of(f, 3).unwrap_or_default());
            let num = if places == DECIMAL_PLACES_AUTO { if thousands { "#,##0.00".to_owned() } else { "0.00".to_owned() } } else { digits(thousands) };
            let pos = if sym.is_empty() { num } else { format!("{} {num}", quoted(&sym)) };
            Some(with_negative(pos))
        }
        258 => Some(if places == DECIMAL_PLACES_AUTO || places == 0 { "0%".to_owned() } else { format!("0.{}%", "0".repeat(places.min(30) as usize)) }),
        259 => Some(format!("0.{}E+00", "0".repeat(if places == DECIMAL_PLACES_AUTO { 2 } else { places.min(30) as usize }))),
        260 => Some("@".to_owned()),
        261 => {
            let pat = text_of(f, 14).unwrap_or_default();
            Some(if pat.is_empty() { "yyyy-mm-dd".to_owned() } else { date_pattern(&pat) })
        }
        262 => Some(match uint_of(f, 11).unwrap_or(0) {
            1 => "# ?/?",
            2 => "# ??/??",
            3 => "# ???/???",
            _ => "# ?/?",
        }
        .to_owned()),
        268 => Some("[h]:mm:ss".to_owned()),
        1 | 263 | 267 | 269 => {
            if ty == 269 {
                notes.insert("a number shown in another base (binary, hex...) is written as a plain number".to_owned());
            }
            None
        }
        _ => None,
    }
}

/// One `FormatStructArchive` of a custom format as an Excel code (a single section).
fn custom_format(def: &[u8], notes: &mut HashSet<String>) -> Option<String> {
    let s = text_of(def, 18)?;
    let ty = uint_of(def, 1).unwrap_or(0);
    if ty == 272 {
        return Some(date_pattern(&s));
    }
    if ty == 271 {
        return Some(s);
    }
    // numbers: Numbers' quote for text is ', and ¤ stands for the currency
    let sym = currency_symbol(&text_of(def, 3).unwrap_or_default());
    let mut code = s.replace('\u{a4}', &if sym.is_empty() { String::new() } else { format!("{}\\ ", quoted(&sym)) });
    if code.contains('\'') {
        code = code.replace('\'', "\"");
    }
    // the thousands separator is a setting of its own: a `,` in the code does nothing when it is off
    if !flag_of(def, 5).unwrap_or(false) {
        let mut inside = false;
        code = code
            .chars()
            .filter(|&c| {
                if c == '"' {
                    inside = !inside;
                }
                inside || c != ','
            })
            .collect();
    }
    let _ = notes;
    Some(code)
}

/// Excel sections from a default format and Numbers' conditions (`<0` and `>=0` around zero, the usual case):
/// `positive;negative[;zero]`. Numbers drops the minus sign when a rule applies, which Excel does in the second
/// section. Conditions on other values become `[<5]a;[>=5]b;default` (Excel keeps two conditions).
fn custom_sections(default: String, conditions: &[(u64, f64, String)], notes: &mut HashSet<String>) -> Option<String> {
    if conditions.is_empty() {
        return Some(default);
    }
    // EQUAL 0, LESS_THAN 1, LESS_THAN_OR_EQUAL_TO 2, GREATER_THAN 3, GREATER_THAN_OR_EQUAL_TO 4
    let on_zero = conditions.iter().all(|(_, v, _)| *v == 0.0);
    let pick = |t: u64| conditions.iter().find(|(c, _, _)| *c == t).map(|(_, _, f)| f.clone());
    if on_zero {
        let neg = pick(1);
        let pos = pick(4).or_else(|| pick(3));
        let zero = pick(0);
        if let (Some(neg), Some(pos)) = (neg.clone(), pos.clone()) {
            return Some(match (zero, pick(4).is_some()) {
                (Some(z), _) => format!("{pos};{neg};{z}"),
                (None, true) => format!("{pos};{neg}"),
                (None, false) => format!("{pos};{neg};{default}"),
            });
        }
        if let Some(neg) = neg {
            return Some(format!("{default};{neg}"));
        }
    }
    let op = |t: u64| match t {
        0 => "=",
        1 => "<",
        2 => "<=",
        3 => ">",
        _ => ">=",
    };
    notes.insert("a custom number format with conditions on values other than zero keeps at most two of them".to_owned());
    let parts: Vec<String> = conditions.iter().take(2).map(|(t, v, f)| format!("[{}{v}]{f}", op(*t))).collect();
    Some(format!("{};{default}", parts.join(";")))
}

// ---- formulas ------------------------------------------------------------------------------------------------

/// A marker for "the worksheet of table N", replaced once every table has its worksheet name.
fn table_marker(i: usize) -> String {
    format!("\u{1}{i}\u{1}")
}

struct Formula<'a> {
    stack: Vec<String>,
    row: usize,
    col: usize,
    uuid_to_table: &'a HashMap<u128, usize>,
}

impl Formula<'_> {
    fn pop(&mut self) -> Result<String, String> {
        self.stack.pop().ok_or_else(|| "formula stack".to_owned())
    }

    fn binary(&mut self, op: &str) -> Result<(), String> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.stack.push(format!("{a}{op}{b}"));
        Ok(())
    }

    fn args(&mut self, n: usize) -> Result<String, String> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.pop()?);
        }
        v.reverse();
        Ok(v.join(","))
    }

    fn cell_ref(&self, node: &[u8]) -> Result<String, String> {
        let prefix = bytes_of(node, 28)
            .and_then(|x| bytes_of(x, 1))
            .and_then(cfuuid_of)
            .map(|u| self.uuid_to_table.get(&u).map(|&i| format!("{}!", table_marker(i))).ok_or("a formula points at a table that is gone"))
            .transpose()?
            .unwrap_or_default();
        let sticky = bytes_of(node, 33);
        let st = |n| sticky.and_then(|s| flag_of(s, n)).unwrap_or(false);
        let at = |col: Option<i64>, col_abs: bool, row: Option<i64>, row_abs: bool| -> Result<String, String> {
            let c = col.map(|c| usize::try_from(c).map_err(|_| "a reference outside the table".to_owned())).transpose()?;
            let r = row.map(|r| usize::try_from(r).map_err(|_| "a reference outside the table".to_owned())).transpose()?;
            Ok(format!("{}{}{}{}", if col_abs && c.is_some() { "$" } else { "" }, c.map(column_letters).unwrap_or_default(), if row_abs && r.is_some() { "$" } else { "" }, r.map(|r| (r + 1).to_string()).unwrap_or_default()))
        };
        if let Some(tract) = bytes_of(node, 40) {
            const NONE_ROW: i64 = 0x7FFF_FFFF;
            const NONE_COL: i64 = 0x7FFF;
            // The begin (or end) of a range along one axis: absolute, relative to the formula's own cell, or "open"
            // (the whole row or column) when it equals `max`.
            let range = |abs_list: u32, rel_list: u32, abs_flag: bool, offset: usize, max: i64, end: bool| -> Option<i64> {
                let absolute = all_bytes_of(tract, abs_list);
                let relative = all_bytes_of(tract, rel_list);
                let val = |m: &[u8]| if end { uint_of(m, 2).or_else(|| uint_of(m, 1)) } else { uint_of(m, 1) };
                if abs_flag {
                    return val(absolute.first()?).map(|v| v as i64);
                }
                if relative.is_empty() {
                    let v = val(absolute.first()?).map(|v| v as i64);
                    return if v == Some(max) { v } else { None };
                }
                // an int32 in a varint: a negative one has all 64 bits
                let rel = val(relative.first()?)?;
                Some(offset as i64 + i64::from(rel as u32 as i32))
            };
            let rb = range(4, 2, st(1), self.row, NONE_ROW, false).ok_or("unreadable range")?;
            let re = range(4, 2, st(3), self.row, NONE_ROW, true).ok_or("unreadable range")?;
            let cb = range(3, 1, st(2), self.col, NONE_COL, false).ok_or("unreadable range")?;
            let ce = range(3, 1, st(4), self.col, NONE_COL, true).ok_or("unreadable range")?;
            let (rb, re) = (if rb == NONE_ROW { None } else { Some(rb) }, if re == NONE_ROW { None } else { Some(re) });
            let (cb, ce) = (if cb == NONE_COL { None } else { Some(cb) }, if ce == NONE_COL { None } else { Some(ce) });
            let a = at(cb, st(2), rb, st(1))?;
            let b = at(ce, st(4), re, st(3))?;
            return Ok(if rb == re && cb == ce { format!("{prefix}{a}") } else { format!("{prefix}{a}:{b}") });
        }
        let row = bytes_of(node, 27);
        let col = bytes_of(node, 26);
        let rv = row.map(|r| {
            let v = zigzag(uint_of(r, 1).unwrap_or(0));
            let abs = flag_of(r, 2).unwrap_or(false);
            (if abs { v } else { self.row as i64 + v }, abs)
        });
        let cv = col.map(|c| {
            let v = zigzag(uint_of(c, 1).unwrap_or(0));
            let abs = flag_of(c, 2).unwrap_or(false);
            (if abs { v } else { self.col as i64 + v }, abs)
        });
        match (rv, cv) {
            (None, None) => Err("an empty reference".to_owned()),
            (r, c) => Ok(format!("{prefix}{}", at(c.map(|x| x.0), c.is_some_and(|x| x.1), r.map(|x| x.0), r.is_some_and(|x| x.1))?)),
        }
    }

    fn node(&mut self, n: &[u8]) -> Result<(), String> {
        let ty = uint_of(n, 1).ok_or("a formula node without a type")?;
        match ty {
            1 => self.binary("+")?,
            2 => self.binary("-")?,
            3 => self.binary("*")?,
            4 => self.binary("/")?,
            5 => self.binary("^")?,
            6 => self.binary("&")?,
            7 => self.binary(">")?,
            8 => self.binary(">=")?,
            9 => self.binary("<")?,
            10 => self.binary("<=")?,
            11 => self.binary("=")?,
            12 => self.binary("<>")?,
            13 => {
                let a = self.pop()?;
                self.stack.push(format!("-{a}"));
            }
            14 | 32 | 33 | 26 | 34 | 35 => {} // plus sign, whitespace, thunk markers
            15 => {
                let a = self.pop()?;
                self.stack.push(format!("{a}%"));
            }
            16 => {
                let id = uint_of(n, 2).ok_or("a function without an id")? as u32;
                let count = uint_of(n, 3).unwrap_or(0) as usize;
                let name = numbers_functions::name(id).ok_or_else(|| format!("function {id} is not known"))?;
                if self.stack.len() < count {
                    return Err(format!("{name}: arguments missing"));
                }
                let args = self.args(count)?;
                self.stack.push(format!("{name}({args})"));
            }
            17 => {
                let text = if uint_of(n, 43) == Some(NUMBER_NODE_DECIMAL_HIGH) {
                    uint_of(n, 42).map(|v| v.to_string())
                } else {
                    f64_of(n, 4).map(|v| format!("{v}"))
                };
                self.stack.push(text.ok_or("a number without a value")?);
            }
            18 | 23 => {
                let b = flag_of(n, 5).or_else(|| flag_of(n, 10)).unwrap_or(false);
                self.stack.push(if b { "TRUE" } else { "FALSE" }.to_owned());
            }
            19 => self.stack.push(format!("\"{}\"", text_of(n, 6).unwrap_or_default().replace('"', "\"\""))),
            20 => {
                let secs = f64_of(n, 7).unwrap_or(0.0);
                let d = iwork::date_text(secs);
                let mut p = d.split(|c: char| !c.is_ascii_digit()).take(3).map(|x| x.parse::<i64>().unwrap_or(0));
                self.stack.push(format!("DATE({},{},{})", p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0)));
            }
            22 => self.stack.push(String::new()),
            24 => {
                let (rows, cols) = (uint_of(n, 12).unwrap_or(1) as usize, uint_of(n, 11).unwrap_or(1) as usize);
                let mut lines = Vec::new();
                for _ in 0..rows {
                    lines.push(self.args(cols)?);
                }
                lines.reverse();
                self.stack.push(format!("{{{}}}", lines.join(";")));
            }
            25 => {
                let count = uint_of(n, 13).unwrap_or(0) as usize;
                let args = self.args(count)?;
                self.stack.push(format!("({args})"));
            }
            29 | 45 => {
                let b = self.pop()?;
                let a = self.pop()?;
                // `Table!A1` : `Table!B2` is `Table!A1:B2`
                let b = match (a.split_once('!'), b.split_once('!')) {
                    (Some((pa, _)), Some((pb, rest))) if pa == pb => rest.to_owned(),
                    _ => b,
                };
                self.stack.push(format!("{a}:{b}"));
            }
            30 | 46 => self.stack.push("#REF!".to_owned()),
            36 | 67 => {
                let r = self.cell_ref(n)?;
                self.stack.push(r);
            }
            other => return Err(format!("formula node type {other} is not supported")),
        }
        Ok(())
    }
}

fn translate(nodes: &[&[u8]], row: usize, col: usize, uuid_to_table: &HashMap<u128, usize>) -> Result<String, String> {
    let mut f = Formula { stack: Vec::new(), row, col, uuid_to_table };
    for n in nodes {
        f.node(n)?;
    }
    let text = f.stack.concat();
    if text.is_empty() { Err("an empty formula".to_owned()) } else { Ok(format!("={text}")) }
}

// ---- styles --------------------------------------------------------------------------------------------------

/// A property of a style or, when the style does not set it, of its parent (a style "is based on" another).
fn inherited<T>(store: &Store, id: u64, get: &dyn Fn(&[u8]) -> Option<T>) -> Option<T> {
    let mut cur = id;
    for _ in 0..12 {
        let (_, body) = store.get(cur)?;
        if let Some(v) = get(body) {
            return Some(v);
        }
        cur = ref_of(bytes_of(body, 1)?, 3)?;
    }
    None
}

fn text_style(store: &Store, id: u64) -> Style {
    let ch = |f: u32| move |b: &[u8]| bytes_of(b, 11).and_then(|c| uint_of(c, f));
    let mut s = Style::default();
    s.bold = inherited(store, id, &ch(1)).is_some_and(|v| v != 0);
    s.italic = inherited(store, id, &ch(2)).is_some_and(|v| v != 0);
    s.underline = inherited(store, id, &ch(11)).is_some_and(|v| v != 0);
    s.strike = inherited(store, id, &ch(12)).is_some_and(|v| v != 0);
    s.size = inherited(store, id, &|b: &[u8]| bytes_of(b, 11).and_then(|c| f32_of(c, 3)));
    s.font = inherited(store, id, &|b: &[u8]| bytes_of(b, 11).and_then(|c| text_of(c, 5)));
    s.color = inherited(store, id, &|b: &[u8]| bytes_of(b, 11).and_then(|c| bytes_of(c, 7)).map(rgb_of)).flatten();
    s.halign = inherited(store, id, &|b: &[u8]| bytes_of(b, 12).and_then(|p| uint_of(p, 1))).map(|v| v as u8);
    s
}

fn cell_style(store: &Store, id: u64, into: &mut Style) {
    let props = |f: u32| move |b: &[u8]| bytes_of(b, 11).and_then(|c| uint_of(c, f));
    into.valign = inherited(store, id, &props(8)).map(|v| v as u8);
    into.wrap = inherited(store, id, &props(3)).is_some_and(|v| v != 0);
    let fill = inherited(store, id, &|b: &[u8]| bytes_of(b, 11).and_then(|c| bytes_of(c, 1)).map(<[u8]>::to_vec));
    if let Some(fill) = fill {
        let color = bytes_of(&fill, 1).map(<[u8]>::to_vec).or_else(|| {
            // a gradient: its first colour is the nearest flat one
            bytes_of(&fill, 2).and_then(|g| all_bytes_of(g, 2).first().and_then(|s| bytes_of(s, 1)).map(<[u8]>::to_vec))
        });
        if let Some(c) = color {
            let alpha = f32_of(&c, 6).unwrap_or(1.0);
            if alpha > 0.02 {
                into.fill = rgb_of(&c);
            }
        }
    }
}

// ---- conditional styles --------------------------------------------------------------------------------------

/// The comparison a conditional style asks, from its predicate's formula: the nodes are `this cell`, a number and an
/// operator (`<`, `<=`, `>`, `>=`, `=`, `<>`). Anything else (text rules, dates, formulas) is None.
fn predicate(nodes: &[&[u8]]) -> Option<(&'static str, f64)> {
    #[derive(Clone, Copy)]
    enum Item {
        This,
        Num(f64),
    }
    let mut stack: Vec<Item> = Vec::new();
    let mut result = None;
    for n in nodes {
        match uint_of(n, 1)? {
            63 | 36 | 27 | 28 | 67 => stack.push(Item::This),
            17 => {
                let v = if uint_of(n, 43) == Some(NUMBER_NODE_DECIMAL_HIGH) { uint_of(n, 42)? as f64 } else { f64_of(n, 4)? };
                stack.push(Item::Num(v));
            }
            13 => {
                if let Some(Item::Num(v)) = stack.pop() {
                    stack.push(Item::Num(-v));
                } else {
                    return None;
                }
            }
            ty @ 7..=12 => {
                let (b, a) = (stack.pop()?, stack.pop()?);
                let (op, v) = match (a, b) {
                    (Item::This, Item::Num(v)) => (ty, v),
                    // `0 < this` is `this > 0`
                    (Item::Num(v), Item::This) => (match ty { 7 => 9, 8 => 10, 9 => 7, 10 => 8, other => other }, v),
                    _ => return None,
                };
                result = Some((
                    match op {
                        7 => "greaterThan",
                        8 => "greaterThanOrEqual",
                        9 => "lessThan",
                        10 => "lessThanOrEqual",
                        11 => "equal",
                        _ => "notEqual",
                    },
                    v,
                ));
            }
            32 | 33 | 26 | 34 | 35 => {}
            _ => return None,
        }
    }
    result
}

/// The rules of the conditional style sets of a table, by the key a cell stores. A rule this reader cannot express is
/// counted in `unsupported`.
fn read_cond_sets(store: &Store, list: Option<u64>, unsupported: &mut usize) -> HashMap<u32, Vec<CondRule>> {
    let mut out = HashMap::new();
    for (key, entry) in list_by_key(store, list) {
        let Some((_, set)) = ref_of(entry, 4).and_then(|id| store.get(id)) else { continue };
        let mut rules = all_bytes_of(set, 2); // the older layout: predicate, cell style, text style
        rules.extend(bytes_of(set, 3).map(|r| all_bytes_of(r, 1)).unwrap_or_default());
        let mut parsed = Vec::new();
        for rule in rules {
            // the predicate holds a formula: its nodes are field 1 of field 1 (older) or field 7 (newer)
            let formula = bytes_of(rule, 1).and_then(|p| bytes_of(p, 1).or_else(|| bytes_of(p, 7)));
            let nodes: Vec<&[u8]> = formula.and_then(|f| bytes_of(f, 1)).map(|arr| all_bytes_of(arr, 1)).unwrap_or_default();
            let mut style = ref_of(rule, 3).map(|id| text_style(store, id)).unwrap_or_default();
            if let Some(id) = ref_of(rule, 2) {
                cell_style(store, id, &mut style);
            }
            match predicate(&nodes) {
                Some((op, value)) => parsed.push(CondRule { op, value, style }),
                None => *unsupported += 1,
            }
        }
        out.insert(u32::try_from(key).unwrap_or(0), parsed);
    }
    out
}

// ---- cells ---------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Raw {
    ty: u8,
    d128: Option<String>,
    double: Option<f64>,
    seconds: Option<f64>,
    string: Option<u32>,
    rich: Option<u32>,
    cell_style: Option<u32>,
    text_style: Option<u32>,
    cond: Option<u32>,
    formula: Option<u32>,
    num_fmt: Option<u32>,
    cur_fmt: Option<u32>,
    date_fmt: Option<u32>,
    dur_fmt: Option<u32>,
    text_fmt: Option<u32>,
    bool_fmt: Option<u32>,
}

fn parse_raw(buf: &[u8]) -> Result<Raw, u8> {
    if buf.len() < 12 {
        return Ok(Raw::default());
    }
    if buf[0] != 5 {
        return Err(buf[0]);
    }
    let flags = i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let mut off = 12usize;
    let mut take = |n: usize| {
        let s = buf.get(off..off + n);
        off += n;
        s
    };
    let mut raw = Raw { ty: buf[1], ..Raw::default() };
    if flags & 0x1 != 0 {
        raw.d128 = take(16).and_then(iwork::decimal128);
    }
    if flags & 0x2 != 0 {
        raw.double = take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
    }
    if flags & 0x4 != 0 {
        raw.seconds = take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
    }
    let mut int = |bit: i32| if flags & bit != 0 { take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) } else { None };
    raw.string = int(0x8);
    raw.rich = int(0x10);
    raw.cell_style = int(0x20);
    raw.text_style = int(0x40);
    raw.cond = int(0x80);
    let _ = int(0x100); // conditional rule style
    raw.formula = int(0x200);
    let _ = (int(0x400), int(0x800), int(0x1000)); // control, formula error, suggest
    raw.num_fmt = int(0x2000);
    raw.cur_fmt = int(0x4000);
    raw.date_fmt = int(0x8000);
    raw.dur_fmt = int(0x10000);
    raw.text_fmt = int(0x20000);
    raw.bool_fmt = int(0x40000);
    Ok(raw)
}

struct TableCtx<'a> {
    store: &'a Store,
    strings: Strings,
    rich_list: u64,
    formats: HashMap<u64, &'a [u8]>,
    /// A cell's style key to the id of the style object (the cell stores the key).
    style_ids: HashMap<u64, u64>,
    formulas: HashMap<u64, Vec<&'a [u8]>>,
    custom: &'a HashMap<u128, &'a [u8]>,
    uuid_to_table: &'a HashMap<u128, usize>,
    header_rows: usize,
    header_cols: usize,
    footer_rows: usize,
    rows: usize,
    /// text style, cell style of the table for body / header row / header column / footer row
    region: [(Option<u64>, Option<u64>); 4],
}

struct Tally {
    versions: usize,
    kinds: usize,
    conditional_unsupported: usize,
    formulas_ok: usize,
    formulas_failed: HashMap<String, usize>,
    format_notes: HashSet<String>,
}

fn region_of(ctx: &TableCtx, row: usize, col: usize) -> usize {
    if row < ctx.header_rows {
        1
    } else if col < ctx.header_cols {
        2
    } else if ctx.footer_rows > 0 && row >= ctx.rows.saturating_sub(ctx.footer_rows) {
        3
    } else {
        0
    }
}

fn read_cell(buf: &[u8], row: usize, col: usize, ctx: &TableCtx, tally: &mut Tally, style_cache: &mut HashMap<(Option<u64>, Option<u64>, usize), Style>, fmt_cache: &mut HashMap<u64, Option<String>>) -> Option<Cell> {
    let raw = match parse_raw(buf) {
        Ok(r) => r,
        Err(_) => {
            tally.versions += 1;
            return None;
        }
    };
    let value = match raw.ty {
        0 => Value::Empty,
        2 | 10 => Value::Number(raw.d128.clone().unwrap_or_else(|| "0".to_owned())),
        3 => Value::Text(ctx.strings.by_key.get(&u64::from(raw.string.unwrap_or(0))).cloned().unwrap_or_default()),
        5 => Value::Date(raw.seconds.unwrap_or(0.0)),
        6 => Value::Bool(raw.double.unwrap_or(0.0) > 0.0),
        7 => Value::Duration(raw.double.unwrap_or(0.0)),
        8 => Value::Error,
        9 => Value::Text(raw.rich.and_then(|k| iwork::rich_text(ctx.store, ctx.rich_list, u64::from(k))).unwrap_or_default()),
        _ => {
            tally.kinds += 1;
            Value::Empty
        }
    };
    let fmt_key = match raw.ty {
        3 | 9 => raw.text_fmt,
        2 | 10 => raw.cur_fmt.or(raw.num_fmt),
        6 => raw.bool_fmt,
        5 => raw.date_fmt,
        7 => raw.dur_fmt.or(raw.num_fmt),
        _ => None,
    };
    let mut numfmt = fmt_key.and_then(|k| {
        let k = u64::from(k);
        fmt_cache.entry(k).or_insert_with(|| ctx.formats.get(&k).and_then(|f| excel_format(f, ctx.custom, &mut tally.format_notes))).clone()
    });
    // a text cell needs no "text" format (and on a formula it would show the formula as text)
    if matches!(value, Value::Text(_)) && numfmt.as_deref() == Some("@") {
        numfmt = None;
    }
    if numfmt.is_none() {
        numfmt = match value {
            Value::Date(s) => Some(if s.rem_euclid(86_400.0) == 0.0 { "yyyy-mm-dd" } else { "yyyy-mm-dd hh:mm:ss" }.to_owned()),
            Value::Duration(_) => Some("[h]:mm:ss".to_owned()),
            _ => None,
        };
    }
    let region = region_of(ctx, row, col);
    let text_key = raw.text_style.and_then(|k| ctx.style_ids.get(&u64::from(k)).copied()).or(ctx.region[region].0);
    let cell_key = raw.cell_style.and_then(|k| ctx.style_ids.get(&u64::from(k)).copied()).or(ctx.region[region].1);
    let style = style_cache
        .entry((text_key, cell_key, region))
        .or_insert_with(|| {
            let mut s = text_key.map(|id| text_style(ctx.store, id)).unwrap_or_default();
            if let Some(id) = cell_key {
                cell_style(ctx.store, id, &mut s);
            }
            s
        })
        .clone();
    let formula = raw.formula.and_then(|k| {
        let nodes = ctx.formulas.get(&u64::from(k))?;
        match translate(nodes, row, col, ctx.uuid_to_table) {
            Ok(f) => {
                tally.formulas_ok += 1;
                Some(f)
            }
            Err(why) => {
                *tally.formulas_failed.entry(why).or_default() += 1;
                None
            }
        }
    });
    Some(Cell { value, formula, numfmt, style, cond: raw.cond })
}

/// A style entry of the table's style list: its `reference` is the style object's id.
fn style_ids(entries: &HashMap<u64, &[u8]>) -> HashMap<u64, u64> {
    entries.iter().filter_map(|(k, e)| Some((*k, ref_of(e, 4)?))).collect()
}

fn read_table(store: &Store, model_id: u64, sheet: &str, custom: &HashMap<u128, &[u8]>, uuid_to_table: &HashMap<u128, usize>, tally: &mut Tally) -> Option<Table> {
    let (_, model) = store.get(model_id)?;
    let name = text_of(model, 8).unwrap_or_default();
    let rows = uint_of(model, 6)? as usize;
    let cols = uint_of(model, 7)? as usize;
    if rows.checked_mul(cols)? > 50_000_000 {
        return None;
    }
    let data = bytes_of(model, 4)?;
    let tiles = bytes_of(data, 3)?;
    let tile_size = uint_of(tiles, 2).filter(|&s| s > 0).unwrap_or(256) as usize;
    // a list entry holds the key, a reference count and, in field 6, the format itself
    let formats: HashMap<u64, &[u8]> = list_by_key(store, ref_of(data, 22).or_else(|| ref_of(data, 11))).into_iter().filter_map(|(k, e)| Some((k, bytes_of(e, 6)?))).collect();
    let style_ids = style_ids(&list_by_key(store, ref_of(data, 5)));
    let cond_sets = read_cond_sets(store, ref_of(data, 18), &mut tally.conditional_unsupported);
    let formulas = list_by_key(store, ref_of(data, 6)).into_iter().filter_map(|(k, e)| Some((k, all_bytes_of(bytes_of(e, 5)?, 1).into_iter().flat_map(|arr| all_bytes_of(arr, 1)).collect::<Vec<_>>()))).collect();
    let ctx = TableCtx {
        store,
        strings: ref_of(data, 4).map(|id| iwork::string_table(store, id)).unwrap_or(Strings { by_key: HashMap::new() }),
        rich_list: ref_of(data, 17).unwrap_or(0),
        formats,
        style_ids,
        formulas,
        custom,
        uuid_to_table,
        header_rows: uint_of(model, 9).unwrap_or(0) as usize,
        header_cols: uint_of(model, 10).unwrap_or(0) as usize,
        footer_rows: uint_of(model, 11).unwrap_or(0) as usize,
        rows,
        // body, header row, header column, footer row: text style (24..27) and cell style (18..21) of the table
        region: [(ref_of(model, 24), ref_of(model, 18)), (ref_of(model, 25), ref_of(model, 19)), (ref_of(model, 26), ref_of(model, 20)), (ref_of(model, 27), ref_of(model, 21))],
    };
    let empty = Cell { value: Value::Empty, formula: None, numfmt: None, style: Style::default(), cond: None };
    let mut cells = vec![vec![empty; cols]; rows];
    let mut style_cache = HashMap::new();
    let mut fmt_cache = HashMap::new();
    for t in all_bytes_of(tiles, 1) {
        let (Some(tile_no), Some(tile_ref)) = (uint_of(t, 1), ref_of(t, 2)) else { continue };
        let Some((_, tile)) = store.get(tile_ref) else { continue };
        for info in all_bytes_of(tile, 5) {
            let row = (tile_no as usize).saturating_mul(tile_size).saturating_add(uint_of(info, 1).unwrap_or(0) as usize);
            if row >= rows {
                continue;
            }
            let (Some(buffer), Some(offsets)) = (bytes_of(info, 6), bytes_of(info, 7)) else { continue };
            let wide = uint_of(info, 8).unwrap_or(0) != 0;
            for (c, cell) in iwork::row_cells(buffer, offsets, wide, cols).into_iter().enumerate() {
                if let Some(cell) = cell
                    && let Some(parsed) = read_cell(cell, row, c, &ctx, tally, &mut style_cache, &mut fmt_cache)
                {
                    cells[row][c] = parsed;
                }
            }
        }
    }

    // Styles of empty cells matter too (a pink header row has pink cells with nothing in them), but a table's drawn
    // size is not its data: trim empty, unstyled rows and columns at the end.
    let blank = |c: &Cell| c.value == Value::Empty && c.style.fill.is_none() && c.numfmt.is_none();
    while cells.last().is_some_and(|r| r.iter().all(blank)) {
        cells.pop();
    }
    let width = cells.iter().map(|r| r.iter().rposition(|c| !blank(c)).map_or(0, |p| p + 1)).max().unwrap_or(0);
    for r in &mut cells {
        r.truncate(width);
    }

    // merged ranges
    let mut merges = Vec::new();
    if let Some(map) = ref_of(data, 13).and_then(|id| store.get(id)) {
        for r in all_bytes_of(&map.1, 1) {
            let origin = bytes_of(r, 1).and_then(|o| iwork::uint_of(o, 1)).unwrap_or(0);
            let size = bytes_of(r, 2).and_then(|o| iwork::uint_of(o, 1)).unwrap_or(0);
            let (c0, r0) = ((origin >> 16) as usize, (origin & 0xFFFF) as usize);
            let (nc, nr) = ((size >> 16) as usize, (size & 0xFFFF) as usize);
            if nr * nc > 1 {
                merges.push([r0, c0, r0 + nr - 1, c0 + nc - 1]);
            }
        }
    }

    // column widths and row heights, in points
    let default_w = f32_of_double(model, 17).unwrap_or(98.0);
    let default_h = f32_of_double(model, 16).unwrap_or(20.0);
    let mut col_widths = vec![default_w; width];
    let mut hidden_cols = Vec::new();
    if let Some(b) = ref_of(data, 2).and_then(|id| store.get(id)) {
        for h in all_bytes_of(&b.1, 2) {
            let (Some(i), size) = (uint_of(h, 1), f32_of(h, 2).unwrap_or(0.0)) else { continue };
            if (i as usize) < col_widths.len() && size > 0.0 {
                col_widths[i as usize] = size;
            }
            if uint_of(h, 3).unwrap_or(0) != 0 && (i as usize) < col_widths.len() {
                hidden_cols.push(i as usize);
            }
        }
    }
    let mut row_heights = vec![default_h; cells.len()];
    let mut hidden_rows = Vec::new();
    if let Some(hs) = bytes_of(data, 1) {
        for bucket in refs_of(hs, 2) {
            let Some((_, b)) = store.get(bucket) else { continue };
            for h in all_bytes_of(b, 2) {
                let (Some(i), size) = (uint_of(h, 1), f32_of(h, 2).unwrap_or(0.0)) else { continue };
                if (i as usize) < row_heights.len() && size > 0.0 {
                    row_heights[i as usize] = size;
                }
                if uint_of(h, 3).unwrap_or(0) != 0 && (i as usize) < row_heights.len() {
                    hidden_rows.push(i as usize);
                }
            }
        }
    }

    Some(Table {
        sheet: sheet.to_owned(),
        name,
        xl_name: String::new(),
        cells,
        merges,
        col_widths,
        row_heights,
        header_rows: ctx.header_rows,
        header_cols: ctx.header_cols,
        hidden_cols,
        hidden_rows,
        cond_sets,
        origin: (0.0, 0.0),
        charts: Vec::new(),
    })
}

fn f32_of_double(buf: &[u8], no: u32) -> Option<f32> {
    f64_of(buf, no).map(|v| v as f32)
}

// ---- the document --------------------------------------------------------------------------------------------

// ---- charts --------------------------------------------------------------------------------------------------

/// A drawable's position and size on the sheet's canvas, in points: (x, y, width, height).
fn geometry(drawable: &[u8]) -> Option<(f32, f32, f32, f32)> {
    let g = bytes_of(bytes_of(drawable, 1)?, 1)?;
    let (pos, size) = (bytes_of(g, 1)?, bytes_of(g, 2)?);
    Some((f32_of(pos, 1)?, f32_of(pos, 2)?, f32_of(size, 1)?, f32_of(size, 2)?))
}

/// Numbers' chart type as GridCraft's (type, subtype); 3D charts are drawn flat, others without an equivalent are the
/// nearest one and the second item says so.
fn chart_kind(t: u64) -> Option<(&'static str, &'static str, Option<&'static str>)> {
    let flat = Some("a 3D chart is drawn flat");
    Some(match t {
        1 => ("column", "", None),
        2 => ("bar", "", None),
        3 => ("line", "", None),
        4 => ("area", "", None),
        5 => ("pie", "", None),
        6 => ("column", "stacked", None),
        7 => ("bar", "stacked", None),
        8 => ("area", "stacked", None),
        9 | 23 => ("scatter", "", None),
        10 => ("column", "", Some("a chart mixing column and line is drawn as columns")),
        11 => ("column", "", Some("a chart with two axes is drawn with one")),
        12 => ("column", "", flat),
        13 => ("bar", "", flat),
        14 => ("line", "", flat),
        15 => ("area", "", flat),
        16 => ("pie", "", flat),
        17 => ("column", "stacked", flat),
        18 => ("bar", "stacked", flat),
        19 => ("area", "stacked", flat),
        20 => ("column", "", None),
        21 => ("bar", "", None),
        22 | 24 => ("bubble", "", None),
        25 => ("doughnut", "", None),
        26 => ("doughnut", "", flat),
        27 => ("radar", "", None),
        _ => return None,
    })
}

/// A chart drawable as a chart: the data Numbers keeps in the chart itself, its type and its place. `origin` is the
/// table's top left corner on the same canvas.
fn read_chart(store: &Store, id: u64, notes: &mut HashSet<String>) -> Option<Chart> {
    let (_, drawable) = store.get(id)?;
    let (x, y, w, h) = geometry(drawable)?;
    let chart = bytes_of(drawable, 10000)?;
    let ty = uint_of(chart, 1)?;
    let Some((kind, subtype, note)) = chart_kind(ty) else {
        notes.insert(format!("a chart of a kind this reader does not know (type {ty}) was left out"));
        return None;
    };
    if let Some(n) = note {
        notes.insert(n.to_owned());
    }
    let grid = bytes_of(chart, 7)?;
    let row_names: Vec<String> = all_bytes_of(grid, 1).into_iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
    let col_names: Vec<String> = all_bytes_of(grid, 2).into_iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
    let values: Vec<Vec<Option<f64>>> = all_bytes_of(grid, 3)
        .into_iter()
        .map(|row| all_bytes_of(row, 1).into_iter().map(|v| f64_of(v, 1).or_else(|| f64_of(v, 4)).or_else(|| f64_of(v, 3))).collect())
        .collect();
    // series_direction 1: a series is a row of the grid; 2: a column
    let by_row = uint_of(chart, 5).unwrap_or(1) != 2;
    let (categories, series) = if by_row {
        let series = values.iter().enumerate().map(|(i, row)| (row_names.get(i).cloned().unwrap_or_default(), row.clone())).collect();
        (col_names, series)
    } else {
        let width = values.iter().map(Vec::len).max().unwrap_or(0);
        let series = (0..width).map(|j| (col_names.get(j).cloned().unwrap_or_default(), values.iter().map(|row| row.get(j).copied().flatten()).collect())).collect();
        (row_names, series)
    };
    Some(Chart { kind, subtype, categories, series, x, y, w, h })
}

/// Where a chart goes on its table's worksheet: beside the table when it overlapped it on Numbers' canvas (a worksheet
/// has cells where Numbers had free space), otherwise at the same distance from the table's corner.
fn place_chart(t: &mut Table, mut c: Chart) -> Chart {
    let (tw, th): (f32, f32) = (t.col_widths.iter().sum(), t.row_heights.iter().sum());
    let (mut x, mut y) = (c.x - t.origin.0, c.y - t.origin.1);
    let overlaps = x < tw && x + c.w > 0.0 && y < th && y + c.h > 0.0;
    if overlaps {
        x = tw + 24.0;
        y = y.max(0.0);
    }
    c.x = x.max(0.0);
    c.y = y.max(0.0);
    c
}

/// A worksheet name Excel accepts: no `[]:*?/\`, at most 31 characters, not empty.
fn sheet_name(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if "[]:*?/\\".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let s = s.trim().trim_matches('\'').trim().to_owned();
    let s: String = s.chars().take(31).collect();
    if s.is_empty() { "Sheet".to_owned() } else { s }
}

fn name_tables(tables: &mut [Table]) {
    let one_per_sheet = (0..tables.len()).all(|i| (0..i).all(|j| tables[j].sheet != tables[i].sheet));
    let mut used: HashSet<String> = HashSet::new();
    for t in tables.iter_mut() {
        let wanted = if one_per_sheet { t.sheet.clone() } else { format!("{} - {}", t.sheet, t.name) };
        let mut name = sheet_name(&wanted);
        let mut n = 2;
        while used.contains(&name.to_lowercase()) {
            let tail = format!(" ({n})");
            name = format!("{}{tail}", sheet_name(&wanted).chars().take(31 - tail.chars().count()).collect::<String>());
            n += 1;
        }
        used.insert(name.to_lowercase());
        t.xl_name = name;
    }
}

/// Reads a `.numbers` file into a workbook.
pub fn read_workbook(path: &Path) -> Result<Workbook, String> {
    let store = iwork::open_store(path, "Numbers")?;
    let (_, doc) = store.get(1).ok_or("This Numbers file has no document part.")?;

    // custom formats by uuid
    let mut custom: HashMap<u128, &[u8]> = HashMap::new();
    if let Some((_, list)) = ref_of(doc, 9).or_else(|| bytes_of(doc, 8).and_then(|s| ref_of(s, 12))).and_then(|id| store.get(id)) {
        for (u, c) in all_bytes_of(list, 1).into_iter().zip(all_bytes_of(list, 2)) {
            if let Some(u) = uuid_of(u) {
                custom.insert(u, c);
            }
        }
    }

    // tables in document order (with the sheet each is on and where it sits), and the charts of each sheet
    let mut refs: Vec<(usize, String, u64, (f32, f32))> = Vec::new();
    let mut chart_refs: Vec<(usize, u64)> = Vec::new();
    for (sheet_idx, sheet_id) in refs_of(doc, 1).into_iter().enumerate() {
        let Some((_, sheet)) = store.get(sheet_id) else { continue };
        let sheet_name = text_of(sheet, 1).unwrap_or_default();
        for d in refs_of(sheet, 2) {
            let Some((ty, info)) = store.get(d) else { continue };
            if *ty == TYPE_TABLE_INFO
                && let Some(model) = ref_of(info, 2)
            {
                refs.push((sheet_idx, sheet_name.clone(), model, geometry(info).map_or((0.0, 0.0), |g| (g.0, g.1))));
            } else if *ty == TYPE_CHART_DRAWABLE {
                chart_refs.push((sheet_idx, d));
            }
        }
    }

    // a formula names a table by a uuid that is not the table's own: the formula owner's base uuid
    let mut owner_base: HashMap<u128, u128> = HashMap::new();
    for (_, ty, body) in store.all() {
        if ty == TYPE_FORMULA_OWNER && uint_of(body, 3).unwrap_or(0) == OWNER_KIND_HAUNTED
            && let (Some(owner), Some(base)) = (bytes_of(body, 1).and_then(uuid_of), bytes_of(body, 12).and_then(uuid_of))
        {
            owner_base.insert(owner, base);
        }
    }
    let mut uuid_to_table: HashMap<u128, usize> = HashMap::new();
    for (i, (_, _, model_id, _)) in refs.iter().enumerate() {
        let owner = store.get(*model_id).and_then(|(_, m)| bytes_of(m, 84)).and_then(|h| bytes_of(h, 1)).and_then(uuid_of);
        if let Some(base) = owner.and_then(|o| owner_base.get(&o)) {
            uuid_to_table.insert(*base, i);
        }
    }

    let mut wb = Workbook::default();
    let mut tally = Tally { versions: 0, kinds: 0, conditional_unsupported: 0, formulas_ok: 0, formulas_failed: HashMap::new(), format_notes: HashSet::new() };
    let mut first_table_of_sheet: HashMap<usize, usize> = HashMap::new();
    for (sheet_idx, sheet, model_id, origin) in &refs {
        if let Some(mut t) = read_table(&store, *model_id, sheet, &custom, &uuid_to_table, &mut tally) {
            t.origin = *origin;
            first_table_of_sheet.entry(*sheet_idx).or_insert(wb.tables.len());
            wb.tables.push(t);
        }
    }
    if wb.tables.is_empty() {
        return Err("No table found in this Numbers file.".to_owned());
    }
    name_tables(&mut wb.tables);
    let mut chart_notes: HashSet<String> = HashSet::new();
    for (sheet_idx, id) in &chart_refs {
        let Some(&ti) = first_table_of_sheet.get(sheet_idx) else { continue };
        if let Some(chart) = read_chart(&store, *id, &mut chart_notes) {
            let placed = place_chart(&mut wb.tables[ti], chart);
            wb.tables[ti].charts.push(placed);
        }
    }
    let mut chart_notes: Vec<String> = chart_notes.into_iter().collect();
    chart_notes.sort();
    wb.notes.extend(chart_notes);
    // now every table has its worksheet name: the formulas can point at them
    let names: Vec<String> = wb.tables.iter().map(|t| format!("'{}'", t.xl_name.replace('\'', "''"))).collect();
    for t in &mut wb.tables {
        for cell in t.cells.iter_mut().flatten() {
            if let Some(f) = &mut cell.formula
                && f.contains('\u{1}')
            {
                let mut out = String::new();
                let mut parts = f.split('\u{1}');
                out += parts.next().unwrap_or("");
                while let (Some(idx), Some(rest)) = (parts.next(), parts.next()) {
                    out += idx.parse::<usize>().ok().and_then(|i| names.get(i)).map_or("#REF!", String::as_str);
                    out += rest;
                }
                *f = out;
            }
        }
    }

    if tally.versions > 0 {
        wb.notes.push(format!("{} cells use a storage version this reader does not know and were left empty.", tally.versions));
    }
    if tally.kinds > 0 {
        wb.notes.push(format!("{} cells are of a kind this reader does not know and were left empty.", tally.kinds));
    }
    if tally.conditional_unsupported > 0 {
        wb.notes.push(format!("{} conditional style rules (text, date or formula conditions) are not carried over.", tally.conditional_unsupported));
    }
    let mut failed: Vec<_> = tally.formulas_failed.iter().collect();
    failed.sort();
    for (why, n) in failed {
        wb.notes.push(format!("{n} formulas were kept as their values: {why}."));
    }
    let mut fmt_notes: Vec<_> = tally.format_notes.into_iter().collect();
    fmt_notes.sort();
    wb.notes.extend(fmt_notes);
    Ok(wb)
}

impl Value {
    /// The value as the CSV and the exact text of the cell.
    pub fn csv(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Number(s) => s.clone(),
            Value::Text(s) => s.clone(),
            Value::Date(s) => iwork::date_text(*s),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            Value::Duration(s) => iwork::duration_text(*s),
            Value::Error => "#ERROR".to_owned(),
        }
    }
}

impl Table {
    /// The table's values as rows of text (for CSV).
    pub fn rows(&self) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = self.cells.iter().map(|r| r.iter().map(|c| c.value.csv()).collect()).collect();
        // as in `iwork::read_numbers`: no trailing empty rows or columns of text
        while rows.last().is_some_and(|r| r.iter().all(String::is_empty)) {
            rows.pop();
        }
        let width = rows.iter().map(|r| r.iter().rposition(|c| !c.is_empty()).map_or(0, |p| p + 1)).max().unwrap_or(0);
        for r in &mut rows {
            r.truncate(width);
        }
        rows
    }
}

// ---- the GridCraft script ------------------------------------------------------------------------------------

pub mod script {
    use super::*;

    pub fn json(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn hex(c: [u8; 3]) -> String {
        format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
    }

    /// One cell as a JSON value of `range.setValues` (`parse:false`: numbers stay numbers, strings stay text, a string
    /// starting with `=` is a formula). A text that starts with `=` returns None: it is entered on its own.
    fn cell_json(cell: &Cell, keep_formula: bool) -> Option<String> {
        if keep_formula && let Some(f) = &cell.formula {
            return Some(json(f));
        }
        Some(match &cell.value {
            Value::Empty => "null".to_owned(),
            Value::Number(s) => {
                if s.parse::<f64>().is_ok_and(f64::is_finite) && s.len() <= 17 { s.clone() } else { json(s) }
            }
            Value::Text(s) => {
                if s.starts_with('=') {
                    return None;
                }
                json(s)
            }
            Value::Date(s) => format!("{}", excel_serial(*s)),
            Value::Duration(s) => format!("{}", s / 86_400.0),
            Value::Bool(b) => b.to_string(),
            Value::Error => json("#N/A"),
        })
    }

    /// Cells with the same look in rectangles: rows of runs, equal runs of neighbouring rows stacked.
    fn rectangles(table: &Table, key: &dyn Fn(&Cell) -> Option<String>) -> Vec<(String, [usize; 4])> {
        let mut by_key: HashMap<String, Vec<Vec<(usize, usize)>>> = HashMap::new();
        for (r, row) in table.cells.iter().enumerate() {
            let mut c = 0;
            while c < row.len() {
                let Some(k) = key(&row[c]) else {
                    c += 1;
                    continue;
                };
                let start = c;
                while c + 1 < row.len() && key(&row[c + 1]).as_deref() == Some(k.as_str()) {
                    c += 1;
                }
                let rows = by_key.entry(k).or_default();
                while rows.len() <= r {
                    rows.push(Vec::new());
                }
                rows[r].push((start, c));
                c += 1;
            }
        }
        let mut out = Vec::new();
        let mut keys: Vec<_> = by_key.keys().cloned().collect();
        keys.sort();
        for k in keys {
            let rows = &by_key[&k];
            let mut r = 0;
            while r < rows.len() {
                if rows[r].is_empty() {
                    r += 1;
                    continue;
                }
                let mut end = r;
                while end + 1 < rows.len() && rows[end + 1] == rows[r] {
                    end += 1;
                }
                for &(c0, c1) in &rows[r] {
                    out.push((k.clone(), [r, c0, end, c1]));
                }
                r = end + 1;
            }
        }
        out
    }

    /// GridCraft's default row height, in points: a row of this height needs no command.
    const GRIDCRAFT_ROW_POINTS: i64 = 15;

    /// Runs of equal neighbours: (first index, last index, value).
    fn runs(v: &[i64]) -> Vec<(usize, usize, i64)> {
        let mut out: Vec<(usize, usize, i64)> = Vec::new();
        for (i, &x) in v.iter().enumerate() {
            match out.last_mut() {
                Some(last) if last.2 == x && last.1 + 1 == i => last.1 = i,
                _ => out.push((i, i, x)),
            }
        }
        out
    }

    /// Sorted indexes as (first, last) spans.
    fn consecutive(v: &[usize]) -> Vec<(usize, usize)> {
        let mut sorted = v.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for i in sorted {
            match out.last_mut() {
                Some(last) if last.1 + 1 == i => last.1 = i,
                _ => out.push((i, i)),
            }
        }
        out
    }

    fn range(a: [usize; 4]) -> String {
        if a[0] == a[2] && a[1] == a[3] { cell_name(a[0], a[1]) } else { format!("{}:{}", cell_name(a[0], a[1]), cell_name(a[2], a[3])) }
    }

    fn line(out: &mut String, command: &str, args: &str) {
        out.push_str(command);
        out.push('=');
        out.push_str(args);
        out.push('\n');
    }

    /// The commands that build `wb` in GridCraft. `drop` holds (table, row, column) of formulas to leave out (the
    /// engine could not reproduce Numbers' value for them: the value stays). `flips` holds (table, chart) of charts whose
    /// series came out as rows where Numbers has columns (or the reverse): they are switched.
    pub fn build(wb: &Workbook, drop: &HashSet<(usize, usize, usize)>, flips: &HashSet<(usize, usize)>) -> String {
        let mut out = String::new();
        line(&mut out, "file.new", "{}");
        for (ti, t) in wb.tables.iter().enumerate() {
            if ti > 0 {
                line(&mut out, "home.insertSheet", "{}");
            }
            line(&mut out, "sheet.rename", &format!("{{\"name\":{}}}", json(&t.xl_name)));
            // Numbers draws its own lines; the worksheet's grid would show through the table
            line(&mut out, "view.gridlines", "{\"on\":false}");
            let mut lonely = Vec::new();
            if !t.cells.is_empty() && t.cells.iter().any(|r| !r.is_empty()) {
                let mut rows = Vec::with_capacity(t.cells.len());
                for (r, row) in t.cells.iter().enumerate() {
                    let cells: Vec<String> = row
                        .iter()
                        .enumerate()
                        .map(|(c, cell)| {
                            cell_json(cell, !drop.contains(&(ti, r, c))).unwrap_or_else(|| {
                                lonely.push((r, c, cell.value.csv()));
                                "null".to_owned()
                            })
                        })
                        .collect();
                    rows.push(format!("[{}]", cells.join(",")));
                }
                line(&mut out, "range.setValues", &format!("{{\"range\":\"A1\",\"parse\":false,\"values\":[{}]}}", rows.join(",")));
            }
            for (r, c, text) in lonely {
                line(&mut out, "cell.set", &format!("{{\"cell\":\"{}\",\"input\":{}}}", cell_name(r, c), json(&format!("'{text}"))));
            }
            // number formats
            for (code, rect) in rectangles(t, &|c| c.numfmt.clone()) {
                line(&mut out, "home.numberFormat", &format!("{{\"range\":\"{}\",\"code\":{}}}", range(rect), json(&code)));
            }
            // fonts, colours, alignment: one rectangle list per property
            let props: [(&str, &dyn Fn(&Cell) -> Option<String>, &dyn Fn(&str) -> String); 12] = [
                ("home.bold", &|c| c.style.bold.then(|| "1".to_owned()), &|_| "{\"on\":true,".to_owned()),
                ("home.italic", &|c| c.style.italic.then(|| "1".to_owned()), &|_| "{\"on\":true,".to_owned()),
                ("home.underline", &|c| c.style.underline.then(|| "1".to_owned()), &|_| "{\"on\":true,".to_owned()),
                ("home.strikethrough", &|c| c.style.strike.then(|| "1".to_owned()), &|_| "{\"on\":true,".to_owned()),
                ("home.fontColor", &|c| c.style.color.map(hex), &|v| format!("{{\"color\":{},", json(v))),
                ("home.fillColor", &|c| c.style.fill.map(hex), &|v| format!("{{\"color\":{},", json(v))),
                ("home.fontSize", &|c| c.style.size.map(|s| format!("{}", s.round())), &|v| format!("{{\"size\":{v},")),
                ("home.fontName", &|c| c.style.font.as_deref().map(font_family), &|v| format!("{{\"name\":{},", json(v))),
                ("home.wrapText", &|c| c.style.wrap.then(|| "1".to_owned()), &|_| "{\"on\":true,".to_owned()),
                ("home.alignLeft", &|c| (c.style.halign == Some(0)).then(|| "1".to_owned()), &|_| "{".to_owned()),
                ("home.alignRight", &|c| (c.style.halign == Some(1)).then(|| "1".to_owned()), &|_| "{".to_owned()),
                ("home.alignCenter", &|c| (c.style.halign == Some(2)).then(|| "1".to_owned()), &|_| "{".to_owned()),
            ];
            for (command, key, open) in props {
                for (value, rect) in rectangles(t, &|c| key(c)) {
                    line(&mut out, command, &format!("{}\"range\":\"{}\"}}", open(&value), range(rect)));
                }
            }
            // conditional styles: cells with the same set in rectangles, then each rule of the set
            for (key, rect) in rectangles(t, &|c| c.cond.map(|k| k.to_string())) {
                let Some(rules) = key.parse::<u32>().ok().and_then(|k| t.cond_sets.get(&k)) else { continue };
                for rule in rules {
                    let mut font = Vec::new();
                    if rule.style.bold {
                        font.push("\"bold\":true".to_owned());
                    }
                    if rule.style.italic {
                        font.push("\"italic\":true".to_owned());
                    }
                    if let Some(c) = rule.style.color {
                        font.push(format!("\"color\":{}", json(&hex(c))));
                    }
                    let mut style = Vec::new();
                    if !font.is_empty() {
                        style.push(format!("\"font\":{{{}}}", font.join(",")));
                    }
                    if let Some(f) = rule.style.fill {
                        style.push(format!("\"fill\":{}", json(&hex(f))));
                    }
                    if style.is_empty() {
                        continue;
                    }
                    line(&mut out, "home.conditionalFormat", &format!("{{\"range\":\"{}\",\"rule\":{{\"type\":\"cellIs\",\"operator\":\"{}\",\"value\":{},\"style\":{{{}}}}}}}", range(rect), rule.op, rule.value, style.join(",")));
                }
            }
            // (bottom is what a spreadsheet does anyway)
            for (command, v) in [("home.alignTop", 0u8), ("home.alignMiddle", 1)] {
                for (_, rect) in rectangles(t, &|c| (c.style.valign == Some(v)).then(|| "1".to_owned())) {
                    line(&mut out, command, &format!("{{\"range\":\"{}\"}}", range(rect)));
                }
            }
            for m in &t.merges {
                line(&mut out, "home.mergeCells", &format!("{{\"range\":\"{}\"}}", range(*m)));
            }
            // widths in pixels (96 dpi), heights in points; neighbours of the same size in one command
            let widths: Vec<i64> = t.col_widths.iter().map(|w| (w * 96.0 / 72.0).round() as i64).collect();
            for (a, b, w) in runs(&widths) {
                line(&mut out, "home.columnWidth", &format!("{{\"cols\":\"{}:{}\",\"width\":{w}}}", column_letters(a), column_letters(b)));
            }
            let heights: Vec<i64> = t.row_heights.iter().map(|h| h.round() as i64).collect();
            for (a, b, h) in runs(&heights) {
                if (h - GRIDCRAFT_ROW_POINTS).abs() > 0 {
                    line(&mut out, "home.rowHeight", &format!("{{\"rows\":\"{}:{}\",\"points\":{h}}}", a + 1, b + 1));
                }
            }
            for (command, list, name) in [("home.hideColumns", &t.hidden_cols, true), ("home.hideRows", &t.hidden_rows, false)] {
                for (a, b) in consecutive(list) {
                    let span = if name { format!("\"cols\":\"{}:{}\"", column_letters(a), column_letters(b)) } else { format!("\"rows\":\"{}:{}\"", a + 1, b + 1) };
                    line(&mut out, command, &format!("{{{span}}}"));
                }
            }
            if t.header_rows > 0 || t.header_cols > 0 {
                line(&mut out, "view.freezePanes", &format!("{{\"cell\":\"{}\"}}", cell_name(t.header_rows, t.header_cols)));
            }
            // on paper: as wide as the table needs, then one page wide
            let total: f32 = t.col_widths.iter().sum();
            if total > 480.0 {
                line(&mut out, "pageLayout.orientation", "{\"orientation\":\"landscape\"}");
            }
            line(&mut out, "pageLayout.scaleToFit", "{\"width\":1,\"height\":0}");
            // charts: their data goes in a block to the right of them (Numbers keeps a copy in the chart), then the chart
            let data_col = t.charts.iter().map(|ch| cell_at(t, ch.x + ch.w, 0.0).1).max().unwrap_or(0) + 2;
            let mut block_row = 0;
            for (k, ch) in t.charts.iter().enumerate() {
                let mut rows = vec![format!("[null{}]", ch.categories.iter().map(|c| format!(",{}", json(c))).collect::<String>())];
                for (name, values) in &ch.series {
                    let cells: String = (0..ch.categories.len().max(values.len())).map(|i| values.get(i).copied().flatten().map_or(",null".to_owned(), |v| format!(",{v}"))).collect();
                    rows.push(format!("[{}{cells}]", json(name)));
                }
                let cols = 1 + ch.categories.len().max(ch.series.iter().map(|s| s.1.len()).max().unwrap_or(0));
                let block = [block_row, data_col, block_row + ch.series.len(), data_col + cols - 1];
                line(&mut out, "range.setValues", &format!("{{\"range\":\"{}\",\"parse\":false,\"values\":[{}]}}", cell_name(block[0], block[1]), rows.join(",")));
                let (row, col) = cell_at(t, ch.x, ch.y);
                let mut args = format!("{{\"range\":\"{}\",\"type\":\"{}\",\"at\":\"{}\",\"width\":{},\"height\":{}", range(block), ch.kind, cell_name(row, col), (ch.w * 96.0 / 72.0).round(), (ch.h * 96.0 / 72.0).round());
                if !ch.subtype.is_empty() {
                    args += &format!(",\"subtype\":\"{}\"", ch.subtype);
                }
                args.push('}');
                line(&mut out, "insert.chart", &args);
                if flips.contains(&(ti, k)) {
                    line(&mut out, "chart.switchRowColumn", "{}");
                }
                block_row += ch.series.len() + 2;
            }
            if !t.charts.is_empty() {
                // the data of the charts is not part of the page: print the table and the charts only
                let bottom = t.charts.iter().map(|ch| cell_at(t, 0.0, ch.y + ch.h).0 + 1).max().unwrap_or(0).max(t.cells.len()).max(1);
                line(&mut out, "pageLayout.printArea", &format!("{{\"range\":\"A1:{}\"}}", cell_name(bottom - 1, data_col.saturating_sub(1))));
            }
        }
        line(&mut out, "sheet.activate", &format!("{{\"sheet\":{}}}", json(&wb.tables[0].xl_name)));
        out
    }

    /// The cell a point of the table's worksheet (in points from its corner) falls in: (row, column). Columns and rows
    /// beyond the table are GridCraft's default size.
    pub fn cell_at(t: &Table, x: f32, y: f32) -> (usize, usize) {
        let walk = |sizes: &[f32], default: f32, at: f32| {
            let (mut i, mut acc) = (0usize, 0.0f32);
            while i < 16_000 {
                let w = sizes.get(i).copied().unwrap_or(default);
                if acc + w > at {
                    break;
                }
                acc += w;
                i += 1;
            }
            i
        };
        (walk(&t.row_heights, 15.0, y), walk(&t.col_widths, 48.0, x))
    }

    /// A font family GridCraft can look up from a PostScript name (`Avenir-Medium` is Avenir).
    fn font_family(ps: &str) -> String {
        ps.split('-').next().unwrap_or(ps).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_cells_are_named_like_a_spreadsheet() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(701), "ZZ");
        assert_eq!(column_letters(702), "AAA");
        assert_eq!(cell_name(0, 0), "A1");
        assert_eq!(cell_name(11, 27), "AB12");
    }

    #[test]
    fn numbers_date_patterns_become_excel_ones() {
        assert_eq!(date_pattern("d MMM yyyy"), "d mmm yyyy");
        assert_eq!(date_pattern("dd/MM/yy HH:mm"), "dd/mm/yy hh:mm");
        assert_eq!(date_pattern("EEEE d MMMM yyyy"), "dddd d mmmm yyyy");
        assert_eq!(date_pattern("h:mm a"), "h:mm AM/PM");
        assert_eq!(date_pattern("yyyy'年'M"), "yyyy\"年\"m");
    }

    fn fmt(ty: u64, places: u64, thousands: bool, neg: u64, code: &str) -> Vec<u8> {
        // a FormatStructArchive by hand: type 1, decimals 2, currency code 3, negative style 4, thousands 5
        let mut b = vec![0x08];
        fn v(n: u64, out: &mut Vec<u8>) {
            let mut n = n;
            loop {
                let byte = (n & 0x7F) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(byte);
                    break;
                }
                out.push(byte | 0x80);
            }
        }
        v(ty, &mut b);
        b.push(0x10);
        v(places, &mut b);
        if !code.is_empty() {
            b.push(0x1A);
            v(code.len() as u64, &mut b);
            b.extend(code.as_bytes());
        }
        b.push(0x20);
        v(neg, &mut b);
        b.push(0x28);
        v(u64::from(thousands), &mut b);
        b
    }

    #[test]
    fn number_formats_follow_numbers_settings() {
        let none = HashMap::new();
        let mut notes = HashSet::new();
        let mut f = |ty, places, th, neg, code: &str| excel_format(&fmt(ty, places, th, neg, code), &none, &mut notes);
        assert_eq!(f(256, 2, true, 0, "").unwrap(), "#,##0.00");
        assert_eq!(f(256, 0, false, 0, "").unwrap(), "0");
        assert_eq!(f(256, 253, false, 0, ""), None, "automatic is General");
        assert_eq!(f(256, 2, true, 1, "").unwrap(), "#,##0.00;[Red]#,##0.00", "red, no minus sign");
        assert_eq!(f(256, 1, false, 2, "").unwrap(), "0.0_);(0.0)");
        assert_eq!(f(257, 2, true, 1, "EUR").unwrap(), "\"€\" #,##0.00;[Red]\"€\" #,##0.00");
        assert_eq!(f(257, 2, false, 3, "USD").unwrap(), "\"$\" 0.00_);[Red](\"$\" 0.00)");
        assert_eq!(f(258, 1, false, 0, "").unwrap(), "0.0%");
        assert_eq!(f(259, 3, false, 0, "").unwrap(), "0.000E+00");
        assert_eq!(f(260, 0, false, 0, "").unwrap(), "@");
    }

    /// Nodes by hand: `SUM(B2:B3)` in C4 is a colon tract and a function; `A1 * 2 + 1`.
    fn node(ty: u64, extra: &[u8]) -> Vec<u8> {
        let mut b = vec![0x08, ty as u8];
        b.extend(extra);
        b
    }

    #[test]
    fn a_formula_is_a_stack_of_nodes() {
        let map = HashMap::new();
        // relative cell reference: row +0, column -1 (sint32 zigzag: -1 is 1, +0 is 0); from cell (row 3, col 2)
        let row = [0xDA, 0x01, 0x02, 0x08, 0x00]; // field 27 (AST_row) = {row: 0}
        let col = [0xD2, 0x01, 0x02, 0x08, 0x01]; // field 26 (AST_column) = {column: -1}
        let mut reference = node(36, &row);
        reference.extend(col);
        let mut number = node(17, &[]);
        number.extend([0xD0, 0x02, 0x02]); // field 42 decimal_low = 2
        number.extend([0xD8, 0x02]); // field 43 decimal_high varint...
        number.extend({
            let mut v = Vec::new();
            let mut n = NUMBER_NODE_DECIMAL_HIGH;
            loop {
                let byte = (n & 0x7F) as u8;
                n >>= 7;
                if n == 0 {
                    v.push(byte);
                    break;
                }
                v.push(byte | 0x80);
            }
            v
        });
        let mul = node(3, &[]);
        let nodes: Vec<&[u8]> = vec![&reference, &number, &mul];
        assert_eq!(translate(&nodes, 3, 2, &map).unwrap(), "=B4*2");
        let unknown = node(99, &[]);
        assert!(translate(&[&unknown[..]], 0, 0, &map).is_err());
        assert!(translate(&[&mul[..]], 0, 0, &map).is_err(), "an operator with nothing to operate on");
    }

    /// With `VOIDFLARE_TEST_NUMBERS` set to a real file: what the reader found, and the script it would run
    /// (written to `VOIDFLARE_TEST_SCRIPT` when that is set).
    #[test]
    fn a_real_workbook_reads() {
        let Some(path) = std::env::var_os("VOIDFLARE_TEST_NUMBERS") else { return };
        let wb = read_workbook(Path::new(&path)).expect("reads");
        for t in &wb.tables {
            println!("== {} / {} -> {:?}: {} rows x {} cols, header rows {} cols {}, merges {:?}", t.sheet, t.name, t.xl_name, t.cells.len(), t.cells.first().map_or(0, Vec::len), t.header_rows, t.header_cols, t.merges);
            println!("   widths {:?} heights {:?}", t.col_widths, t.row_heights);
            for ch in &t.charts {
                println!("   CHART {} {} series={} cats={:?} at=({:.0},{:.0}) size=({:.0},{:.0}) first={:?}", ch.kind, ch.subtype, ch.series.len(), ch.categories, ch.x, ch.y, ch.w, ch.h, ch.series.first());
            }
            for (r, row) in t.cells.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    if cell.value != Value::Empty || cell.style.fill.is_some() {
                        println!("   {} {:?} f={:?} fmt={:?} st={:?}", cell_name(r, c), cell.value, cell.formula, cell.numfmt, cell.style);
                    }
                }
            }
        }
        for n in &wb.notes {
            println!("NOTE: {n}");
        }
        if let Some(out) = std::env::var_os("VOIDFLARE_TEST_SCRIPT") {
            std::fs::write(out, script::build(&wb, &HashSet::new(), &HashSet::new())).unwrap();
        }
    }

    #[test]
    fn worksheet_names_fit_excel() {
        assert_eq!(sheet_name("Budget: 2026/27 [draft]"), "Budget  2026 27  draft");
        assert_eq!(sheet_name(&"x".repeat(40)).chars().count(), 31);
        assert_eq!(sheet_name("   "), "Sheet");
        let mk = |sheet: &str, name: &str| Table { sheet: sheet.into(), name: name.into(), xl_name: String::new(), cells: vec![], merges: vec![], col_widths: vec![], row_heights: vec![], header_rows: 0, header_cols: 0, hidden_cols: vec![], hidden_rows: vec![], cond_sets: HashMap::new(), origin: (0.0, 0.0), charts: vec![] };
        let mut one = vec![mk("Foglio 1", "Tabella 1"), mk("Foglio 2", "Tabella 1")];
        name_tables(&mut one);
        assert_eq!(one[0].xl_name, "Foglio 1");
        let mut two = vec![mk("Foglio 1", "A"), mk("Foglio 1", "B")];
        name_tables(&mut two);
        assert_eq!((two[0].xl_name.as_str(), two[1].xl_name.as_str()), ("Foglio 1 - A", "Foglio 1 - B"));
    }

    #[test]
    fn the_script_carries_values_looks_and_formulas() {
        let c = |value: Value, formula: Option<&str>, numfmt: Option<&str>| Cell { value, formula: formula.map(str::to_owned), numfmt: numfmt.map(str::to_owned), style: Style { bold: true, fill: Some([238, 63, 150]), ..Style::default() }, cond: None };
        let t = Table {
            sheet: "S".into(),
            name: "T".into(),
            xl_name: "S".into(),
            cells: vec![vec![c(Value::Text("Voce".into()), None, None), c(Value::Number("-24.95".into()), None, Some("0.00"))], vec![c(Value::Text("=sembra".into()), None, None), c(Value::Number("18.6".into()), Some("=SUM(B1:B1)"), None)]],
            merges: vec![],
            col_widths: vec![72.0, 72.0],
            row_heights: vec![20.0, 20.0],
            header_rows: 1,
            header_cols: 0,
            hidden_cols: vec![],
            hidden_rows: vec![],
            cond_sets: HashMap::new(),
            origin: (0.0, 0.0),
            charts: vec![],
        };
        let wb = Workbook { tables: vec![t], notes: vec![] };
        let s = script::build(&wb, &HashSet::new(), &HashSet::new());
        assert!(s.contains("range.setValues={\"range\":\"A1\",\"parse\":false,\"values\":[[\"Voce\",-24.95],[null,\"=SUM(B1:B1)\"]]}"), "{s}");
        assert!(s.contains("cell.set={\"cell\":\"A2\",\"input\":\"'=sembra\"}"), "text starting with = is entered on its own: {s}");
        assert!(s.contains("home.numberFormat={\"range\":\"B1\",\"code\":\"0.00\"}"));
        assert!(s.contains("home.fillColor={\"color\":\"#EE3F96\",\"range\":\"A1:B2\"}"), "{s}");
        assert!(s.contains("home.columnWidth={\"cols\":\"A:B\",\"width\":96}"), "neighbours of one width are one command: {s}");
        assert!(s.contains("view.freezePanes={\"cell\":\"A2\"}"));
        let dropped = script::build(&wb, &HashSet::from([(0, 1, 1)]), &HashSet::new());
        assert!(dropped.contains("[null,18.6]"), "a dropped formula leaves its value: {dropped}");
    }
}
