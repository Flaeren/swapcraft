//! Excel to PDF's conversions on GridCraft: PDF, XLSX and CSV, with two things the engine does not do alone.
//!
//! - A workbook with several sheets becomes one CSV per sheet beside it (`Budget - January.csv`): GridCraft's
//!   `convert` writes only the active sheet and drops the others without a word.
//! - A CSV is read the way it was meant before GridCraft sees it. A CSV saved by Excel in Italian, Spanish, French
//!   or German uses `;`, a decimal comma and day-first dates: GridCraft would keep `12,5` as text and read
//!   `05/11/2026` as 11 May. Codes with a leading zero (`00123`) and numbers longer than 15 digits stay text in every CSV, since a
//!   number would lose the zeros or the last digits. A file with nothing to change reaches GridCraft untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::job::{self, Spec};

/// Converts one spreadsheet into `spec.out_ext` (`spec.cli` is GridCraft), into `out_dir`.
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    convert_noted(spec, src, out_dir, &|_| {})
}

/// [`convert`] that says, through `note`, which file each sheet went to when there are several.
pub fn convert_noted(spec: &Spec, src: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
    let result = convert_in(&tmp, spec, src, out_dir, note);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn convert_in(tmp: &Path, spec: &Spec, src: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("table").to_owned();
    if ext == "numbers" {
        return convert_numbers(tmp, spec, src, &stem, out_dir, note);
    }
    // The tidied copy keeps the file's name, so the result is named after it.
    let mut source = src.to_path_buf();
    if matches!(ext.as_str(), "csv" | "tsv") {
        let bytes = fs::read(src).map_err(|e| e.to_string())?;
        if let Some(text) = tidy_csv(&bytes, ext == "tsv") {
            source = tmp.join(format!("{stem}.csv"));
            fs::write(&source, text).map_err(|e| e.to_string())?;
        }
    }
    if !matches!(spec.out_ext.as_str(), "csv" | "tsv") || ext != "xlsx" {
        return job::convert_one(spec, &source, out_dir);
    }
    let sheets = sheets_with_cells(&spec.cli, &source, tmp)?;
    if sheets.len() < 2 {
        // One sheet with data: that one, even when another (empty) sheet is the active one.
        let args = sheets.first().map(|s| vec!["--sheet".to_owned(), s.clone()]).unwrap_or_default();
        return job::convert_one(&Spec { extra_args: [spec.extra_args.clone(), args].concat(), ..spec.clone() }, &source, out_dir);
    }
    let mut written = Vec::new();
    for (i, sheet) in sheets.iter().enumerate() {
        let dir = tmp.join(format!("sheet{i}"));
        let one = Spec { extra_args: [spec.extra_args.clone(), vec!["--sheet".to_owned(), sheet.clone()]].concat(), ..spec.clone() };
        let made = job::convert_one(&one, &source, &dir)?;
        let name = format!("{stem} - {}.{}", file_safe(sheet), spec.out_ext);
        let out = job::deliver(&made, out_dir, &name)?;
        note(sheet_went_to(sheet, &out.file_name().unwrap_or_default().to_string_lossy()));
        written.push(out);
    }
    Ok(written.swap_remove(0))
}

/// An Apple Numbers file (see [`crate::numbers`]).
///
/// - To CSV: one file per table, the values exactly as Numbers holds them.
/// - To XLSX or PDF: one workbook with a sheet per table, built by GridCraft from a script with the values, the number
///   formats, fonts, fills, merged cells, widths, conditional styles and formulas. A formula stays a formula only when
///   GridCraft's result equals the one Numbers saved; otherwise the cell keeps Numbers' value and the log says so.
/// - If the workbook cannot be built the values still go through as CSV, with a note.
fn convert_numbers(tmp: &Path, spec: &Spec, src: &Path, stem: &str, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let wb = crate::numbers::read_workbook(src)?;
    for n in &wb.notes {
        note(n.clone());
    }
    if spec.out_ext != "csv" {
        match numbers_workbook(tmp, spec, &wb, stem, out_dir, note) {
            Ok(out) => return Ok(out),
            Err(why) => note(format!("The formatted workbook could not be built ({why}); the values are written without their formatting.")),
        }
    }
    numbers_via_csv(tmp, spec, &wb, stem, out_dir, note)
}

fn numbers_workbook(tmp: &Path, spec: &Spec, wb: &crate::numbers::Workbook, stem: &str, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    use std::collections::HashSet;
    let work = tmp.join("workbook");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let xlsx = work.join(format!("{stem}.xlsx"));
    let mut dropped: HashSet<(usize, usize, usize)> = HashSet::new();
    let mut flips: HashSet<(usize, usize)> = HashSet::new();
    run_script(&spec.cli, &crate::numbers::script::build(wb, &dropped, &flips), &xlsx, &work)?;
    // A formula is kept only if GridCraft computes what Numbers saved; a chart's series must be what Numbers has.
    let wrong = cells_that_differ(&spec.cli, wb, &xlsx, &work, false)?;
    let flipped = charts_to_flip(&spec.cli, wb, &xlsx, &work)?;
    if !wrong.is_empty() || !flipped.is_empty() {
        for (ti, r, c) in &wrong {
            note(format!("formula kept as its value: {} {}", wb.tables[*ti].xl_name, crate::numbers::cell_name(*r, *c)));
        }
        dropped.extend(wrong);
        flips.extend(flipped);
        run_script(&spec.cli, &crate::numbers::script::build(wb, &dropped, &flips), &xlsx, &work)?;
    }
    let made = if spec.out_ext == "xlsx" {
        xlsx
    } else {
        let out = work.join(format!("{stem}.{}", spec.out_ext));
        run_script(&spec.cli, &crate::numbers::script::build(wb, &dropped, &flips), &out, &work)?;
        out
    };
    job::deliver(&made, out_dir, &format!("{stem}.{}", spec.out_ext))
}

/// The charts (table, index) whose series GridCraft took by rows where Numbers has them by columns, or the reverse:
/// the number of series in the built workbook is not the number Numbers has.
fn charts_to_flip(cli: &Path, wb: &crate::numbers::Workbook, xlsx: &Path, work: &Path) -> Result<Vec<(usize, usize)>, String> {
    if wb.tables.iter().all(|t| t.charts.is_empty()) {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new(cli);
    cmd.arg("info").arg("--json").arg(std::path::absolute(xlsx).unwrap_or_else(|_| xlsx.to_path_buf()));
    let info: serde_json::Value = serde_json::from_str(&job::run_capture(&mut cmd, work)?).map_err(|e| format!("GridCraft did not describe the workbook: {e}"))?;
    let sheets = info["sheets"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut flips = Vec::new();
    for (ti, t) in wb.tables.iter().enumerate() {
        let Some(sheet) = sheets.iter().find(|s| s["name"].as_str() == Some(t.xl_name.as_str())) else { continue };
        let built = sheet["charts"].as_array().map(Vec::as_slice).unwrap_or_default();
        for (k, ch) in t.charts.iter().enumerate() {
            if built.get(k).and_then(|b| b["series"].as_u64()).is_some_and(|n| n as usize != ch.series.len()) {
                flips.push((ti, k));
            }
        }
    }
    Ok(flips)
}

/// Runs `script` in GridCraft and writes the workbook (or PDF, by the extension) to `out`.
fn run_script(cli: &Path, script: &str, out: &Path, work: &Path) -> Result<(), String> {
    let path = work.join("workbook.jsonl");
    fs::write(&path, script).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(out);
    let mut cmd = Command::new(cli);
    cmd.arg("run").arg("--script").arg(std::path::absolute(&path).unwrap_or(path)).arg("--out").arg(std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf()));
    job::run_capture(&mut cmd, work)?;
    if out.is_file() { Ok(()) } else { Err("GridCraft wrote no file".to_owned()) }
}

/// The cells (table, row, column) whose value in the built workbook is not the one Numbers saved: the formula cells,
/// or with `all` every cell. The workbook is opened again with a scratch sheet whose cells point at the ones to check
/// (as plain numbers, text or TRUE/FALSE, so no number format is applied) and that sheet is exported as CSV.
fn cells_that_differ(cli: &Path, wb: &crate::numbers::Workbook, xlsx: &Path, work: &Path, all: bool) -> Result<Vec<(usize, usize, usize)>, String> {
    use crate::numbers::{Value, script::json};
    enum Expect {
        Number(f64),
        Text(String),
    }
    let mut asks: Vec<(usize, usize, usize, String, Expect)> = Vec::new();
    for (ti, t) in wb.tables.iter().enumerate() {
        for (r, row) in t.cells.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if !all && cell.formula.is_none() {
                    continue;
                }
                let at = format!("'{}'!{}", t.xl_name.replace('\'', "''"), crate::numbers::cell_name(r, c));
                let ask = match &cell.value {
                    Value::Number(s) => s.parse::<f64>().ok().map(|v| (format!("=1*{at}"), Expect::Number(v))),
                    Value::Date(s) => Some((format!("=1*{at}"), Expect::Number(crate::numbers::excel_serial(*s)))),
                    Value::Duration(s) => Some((format!("=1*{at}"), Expect::Number(s / 86_400.0))),
                    Value::Bool(b) => Some((format!("=IF({at},\"TRUE\",\"FALSE\")"), Expect::Text(if *b { "TRUE" } else { "FALSE" }.to_owned()))),
                    Value::Text(s) => Some((format!("={at}&\"\""), Expect::Text(s.clone()))),
                    Value::Empty | Value::Error => None,
                };
                if let Some((f, e)) = ask {
                    asks.push((ti, r, c, f, e));
                }
            }
        }
    }
    if asks.is_empty() {
        return Ok(Vec::new());
    }
    // A text starting with `=` would be taken for a formula in the scratch sheet: those are not asked.
    asks.retain(|(_, _, _, _, e)| !matches!(e, Expect::Text(t) if t.starts_with('=')));
    let rows: Vec<String> = asks.iter().map(|(_, _, _, f, _)| format!("[{}]", json(f))).collect();
    let script = format!(
        "file.open={{\"path\":{}}}
home.insertSheet={{}}
sheet.rename={{\"name\":\"zz check\"}}
range.setValues={{\"range\":\"A1\",\"parse\":false,\"values\":[{}]}}
",
        json(&std::path::absolute(xlsx).unwrap_or_else(|_| xlsx.to_path_buf()).to_string_lossy()),
        rows.join(",")
    );
    let csv = work.join("check.csv");
    run_script(cli, &script, &csv, work)?;
    let grid = parse(&decode(&fs::read(&csv).map_err(|e| e.to_string())?), ',');
    let mut wrong = Vec::new();
    for (i, (ti, r, c, _, expect)) in asks.iter().enumerate() {
        let got = grid.get(i).and_then(|x| x.first()).map(String::as_str).unwrap_or("");
        let same = match expect {
            Expect::Number(v) => got.trim().parse::<f64>().is_ok_and(|g| (g - v).abs() <= 1e-9 * v.abs().max(1.0)),
            Expect::Text(t) => got == t,
        };
        if !same {
            wrong.push((*ti, *r, *c));
        }
    }
    Ok(wrong)
}

/// The values of every table as CSV files (one per table), then on as CSV.
fn numbers_via_csv(tmp: &Path, spec: &Spec, wb: &crate::numbers::Workbook, stem: &str, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let tables = &wb.tables;
    let one_per_sheet = tables.iter().enumerate().all(|(i, t)| tables.iter().take(i).all(|o| o.sheet != t.sheet));
    let mut used: Vec<String> = Vec::new();
    let mut written = Vec::new();
    for t in tables {
        let mut name = match (tables.len(), one_per_sheet) {
            (1, _) => stem.to_owned(),
            (_, true) => format!("{stem} - {}", file_safe(&t.sheet)),
            _ => format!("{stem} - {} - {}", file_safe(&t.sheet), file_safe(&t.name)),
        };
        if used.contains(&name) {
            name = format!("{name} ({})", used.len() + 1);
        }
        used.push(name.clone());
        let dir = tmp.join(format!("numbers{}", used.len()));
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let csv_path = dir.join(format!("{name}.csv"));
        fs::write(&csv_path, crate::iwork::csv(&t.rows())).map_err(|e| e.to_string())?;
        let out = if spec.out_ext == "csv" {
            job::deliver(&csv_path, out_dir, &format!("{name}.csv"))?
        } else {
            convert_in(&dir, spec, &csv_path, out_dir, note)?
        };
        if tables.len() > 1 {
            note(sheet_went_to(&format!("{} / {}", t.sheet, t.name), &out.file_name().unwrap_or_default().to_string_lossy()));
        }
        written.push(out);
    }
    Ok(written.swap_remove(0))
}

/// "sheet X → file", translated where the program has the catalog (the command line and the MCP server do not).
fn sheet_went_to(sheet: &str, file: &str) -> String {
    let said = crate::tf("sheet.went_to", &[("sheet", sheet), ("file", file)]);
    if said.starts_with("sheet.went_to") { format!("sheet {sheet} → {file}") } else { said }
}

/// The names of the sheets that hold something, in workbook order (GridCraft's `info --json`).
fn sheets_with_cells(cli: &Path, src: &Path, tmp: &Path) -> Result<Vec<String>, String> {
    let src = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let mut cmd = Command::new(cli);
    cmd.arg("info").arg("--json").arg(&src);
    let printed = job::run_capture(&mut cmd, tmp)?;
    let info: serde_json::Value = serde_json::from_str(&printed).map_err(|e| format!("GridCraft did not list the sheets: {e}"))?;
    let sheets = info["sheets"].as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(sheets.iter().filter(|s| s["cells"].as_u64().unwrap_or(0) > 0).filter_map(|s| s["name"].as_str().map(str::to_owned)).collect())
}

/// A sheet name as part of a file name: the characters Windows refuses become `_` (Excel allows `"`, `<`, `>`, `|`).
fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_owned();
    if s.is_empty() { "sheet".to_owned() } else { s }
}

/// The CSV rewritten (UTF-8, comma-separated) so GridCraft reads it as meant, or `None` when it already does.
fn tidy_csv(bytes: &[u8], tab: bool) -> Option<String> {
    let text = decode(bytes);
    let delim = if tab { '\t' } else { delimiter(&text) };
    let mut rows = parse(&text, delim);
    let mut changed = false;
    if delim == ';' {
        changed |= european_columns(&mut rows);
    }
    for cell in rows.iter_mut().flatten() {
        if keep_as_text(cell) {
            cell.insert(0, '\'');
            changed = true;
        }
    }
    changed.then(|| write(&rows))
}

/// UTF-8 (a byte order mark dropped), or else Windows-1252, what Excel writes in Western Europe. GridCraft reads both
/// (checked on the bytes it stores); the rewritten copy is always UTF-8.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    // 0x80-0x9F differ from Latin-1; the five codes Windows-1252 leaves unused stay as their Latin-1 controls.
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{9D}', 'ž', 'Ÿ',
    ];
    bytes.iter().map(|&b| if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { char::from(b) }).collect()
}

/// `;` or a tab when the first line has more of it than commas (outside quotes), else `,`.
fn delimiter(text: &str) -> char {
    let mut counts = [0usize; 3];
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' | '\r' if !quoted => break,
            ',' if !quoted => counts[0] += 1,
            ';' if !quoted => counts[1] += 1,
            '\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    if counts[1] > counts[0] && counts[1] >= counts[2] {
        ';'
    } else if counts[2] > counts[0] {
        '\t'
    } else {
        ','
    }
}

/// Rows of fields: quotes around a field, `""` inside one, line breaks inside quotes, CRLF or LF.
fn parse(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let (mut quoted, mut at_start) = (false, true);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            c if c == delim => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            c => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() || !at_start {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn write(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        let fields: Vec<String> = row
            .iter()
            .map(|f| if f.contains([',', '"', '\n', '\r']) { format!("\"{}\"", f.replace('"', "\"\"")) } else { f.clone() })
            .collect();
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// Columns of a `;` file whose every value (a heading aside) is a number with a decimal comma or a day-first date
/// are rewritten as GridCraft reads them (`1.234,5` → `1234.5`, `05/11/2026` → `2026-11-05`). A column with one
/// value that is neither stays as it is, so a column of codes or of mixed text is never touched. `true` if any did.
fn european_columns(rows: &mut [Vec<String>]) -> bool {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut changed = false;
    for col in 0..width {
        for read in [european_number as fn(&str) -> Option<String>, day_first_date] {
            let values: Vec<(usize, Option<String>)> = rows
                .iter()
                .enumerate()
                .filter_map(|(r, row)| row.get(col).filter(|v| !v.trim().is_empty()).map(|v| (r, read(v.trim()))))
                .collect();
            // the first row may be the heading
            let body = if values.first().is_some_and(|(r, v)| *r == 0 && v.is_none()) { &values[1..] } else { &values[..] };
            if body.is_empty() || body.iter().any(|(_, v)| v.is_none()) {
                continue;
            }
            for (r, v) in body {
                let cell = &mut rows[*r][col];
                if let Some(v) = v
                    && cell.trim() != v
                {
                    *cell = v.clone();
                    changed = true;
                }
            }
            break;
        }
    }
    changed
}

/// `-1.234,56` → `-1234.56`, `12,5%` → `12.5%`, `7` → `7`. Not a code with a leading zero (`0123`), and the dots
/// must group thousands (`1.5` is not an Italian number).
fn european_number(s: &str) -> Option<String> {
    let (sign, rest) = match s.strip_prefix(['-', '+']) {
        Some(r) => (if s.starts_with('-') { "-" } else { "" }, r),
        None => ("", s),
    };
    let (rest, percent) = match rest.strip_suffix('%') {
        Some(r) => (r, "%"),
        None => (rest, ""),
    };
    let (int, frac) = match rest.split_once(',') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if frac.is_some_and(|f| !digits(f)) {
        return None;
    }
    let groups: Vec<&str> = int.split('.').collect();
    let grouped = groups.len() > 1 && (1..=3).contains(&groups[0].len()) && groups[1..].iter().all(|g| g.len() == 3);
    if !groups.iter().all(|g| digits(g)) || (groups.len() > 1 && !grouped) {
        return None;
    }
    let whole: String = groups.concat();
    if whole.len() > 1 && whole.starts_with('0') && groups.len() == 1 {
        return None;
    }
    Some(match frac {
        Some(f) => format!("{sign}{whole}.{f}{percent}"),
        None => format!("{sign}{whole}{percent}"),
    })
}

/// `5/11/2026`, `05/11/2026` or `05.11.2026` (day, month, year) → `2026-11-05`, if that day exists.
fn day_first_date(s: &str) -> Option<String> {
    let sep = if s.contains('/') { '/' } else { '.' };
    let mut parts = s.split(sep);
    let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || d.len() > 2 || m.len() > 2 || y.len() != 4 {
        return None;
    }
    let (d, m, y): (u32, u32, u32) = (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=days).contains(&d).then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// Digits that a number would spoil: a leading zero (`00123`, a postcode or an article code) or more than 15 of them
/// (a card or account number: a spreadsheet keeps 15 significant digits). `'` in front keeps them as text.
fn keep_as_text(cell: &str) -> bool {
    let all_digits = !cell.is_empty() && cell.bytes().all(|b| b.is_ascii_digit());
    all_digits && ((cell.len() > 1 && cell.starts_with('0')) || cell.len() > 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::GRIDCRAFT;

    #[test]
    fn italian_numbers_and_dates() {
        assert_eq!(european_number("12,5").as_deref(), Some("12.5"));
        assert_eq!(european_number("-1.234,56").as_deref(), Some("-1234.56"));
        assert_eq!(european_number("1.234").as_deref(), Some("1234"), "a dot groups thousands in a ; file");
        assert_eq!(european_number("12,5%").as_deref(), Some("12.5%"));
        assert_eq!(european_number("0,5").as_deref(), Some("0.5"));
        for no in ["1.5", "00123", "12,5 kg", "1,2,3", "abc", "", "1.23,4.5"] {
            assert_eq!(european_number(no), None, "{no}");
        }
        assert_eq!(day_first_date("05/11/2026").as_deref(), Some("2026-11-05"));
        assert_eq!(day_first_date("29/2/2024").as_deref(), Some("2024-02-29"));
        assert_eq!(day_first_date("05.11.2026").as_deref(), Some("2026-11-05"), "German Excel");
        assert_eq!(day_first_date("5/11.2026"), None);
        assert_eq!(day_first_date("29/02/2026"), None);
        assert_eq!(day_first_date("11/31/2026"), None);
    }

    #[test]
    fn an_italian_excel_csv_is_rewritten_and_codes_stay_text() {
        let csv = "nome;importo;codice;data;nota\r\nNiccol\u{f2};12,5;00123;05/11/2026;a, b\r\nBea;1.234,56;00124;16/11/2026;\"c;d\"\r\n";
        let mut bytes: Vec<u8> = Vec::new();
        for c in csv.chars() {
            bytes.push(u8::try_from(u32::from(c)).unwrap()); // Latin-1 = Windows-1252 for these
        }
        let out = tidy_csv(&bytes, false).unwrap();
        assert_eq!(
            out,
            "nome,importo,codice,data,nota\r\nNiccol\u{f2},12.5,'00123,2026-11-05,\"a, b\"\r\nBea,1234.56,'00124,2026-11-16,c;d\r\n"
        );
        // A column with one value that is not a number keeps all its values.
        assert_eq!(tidy_csv("a;b\n12,5;x\nfoo;y\n".as_bytes(), false), None, "nothing to change: the file goes as it is");
        let mixed = tidy_csv("a;b;c\n12,5;x;1,5\nfoo;y;2\n".as_bytes(), false).unwrap();
        assert_eq!(mixed, "a,b,c\r\n\"12,5\",x,1.5\r\nfoo,y,2\r\n", "only the column of numbers changes");
    }

    #[test]
    fn a_plain_csv_is_left_alone_unless_a_code_needs_keeping() {
        assert_eq!(tidy_csv(b"a,b\n1,2.5\n", false), None);
        assert_eq!(tidy_csv("\u{feff}a,b\n1,2\n".as_bytes(), false), None, "a byte order mark alone changes nothing");
        assert_eq!(tidy_csv(b"cap,n\n00184,1234567890123456789\n", false).unwrap(), "cap,n\r\n'00184,'1234567890123456789\r\n");
        assert_eq!(tidy_csv(b"a\tb\n1,5\t2\n", true), None, "a tsv with nothing to change goes as it is");
        assert_eq!(tidy_csv(b"a\tb\n007\t2\n", true).unwrap(), "a,b\r\n'007,2\r\n", "else as a csv");
        assert_eq!(tidy_csv(b"a;b\nx;y\n", false), None, "a ; file with no numbers or dates goes as it is");
        assert_eq!(parse("a,\"b\"\"c\nd\",e", ','), vec![vec!["a", "b\"c\nd", "e"]]);
    }

    #[test]
    fn sheet_names_become_file_names() {
        assert_eq!(file_safe("Vendite \"Nord\""), "Vendite _Nord_");
        assert_eq!(file_safe("a<b>|c. "), "a_b__c");
        assert_eq!(file_safe("..."), "sheet");
    }

    /// A workbook with two sheets, the second one active (`convert` alone would write only that one).
    fn two_sheet_xlsx(path: &Path) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opt = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &str| {
            zip.start_file(name, opt).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        };
        let sheet_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
        put(
            "[Content_Types].xml",
            &format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{sheet_type}"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="{sheet_type}"/></Types>"#
            ),
        );
        put(
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        );
        put(
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><bookViews><workbookView activeTab="1"/></bookViews><sheets><sheet name="Gennaio" sheetId="1" r:id="rId1"/><sheet name="Vendite &quot;Nord&quot;" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
        );
        put(
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#,
        );
        let sheet = |a: &str, b: f64| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{a}</t></is></c><c r="B1"><v>{b}</v></c></row></sheetData></worksheet>"#
            )
        };
        put("xl/worksheets/sheet1.xml", &sheet("primo", 12.5));
        put("xl/worksheets/sheet2.xml", &sheet("secondo", 7.0));
        zip.finish().unwrap();
    }

    /// With `VOIDFLARE_TEST_NUMBERS` (a real `.numbers`) and GridCraft: the file goes to CSV, XLSX and PDF, and what the
    /// XLSX holds, read back, equals what the Numbers reader found cell by cell.
    #[test]
    fn real_numbers_file_survives_xlsx_and_pdf() {
        let (Some(path), Some(cli)) = (std::env::var_os("VOIDFLARE_TEST_NUMBERS"), GRIDCRAFT.find()) else { return };
        let src = Path::new(&path);
        let dir = job::scratch_dir().unwrap();
        let mine = crate::iwork::read_numbers(src).unwrap();
        assert_eq!(mine.tables.len(), 1, "this check handles one table");
        let spec = Spec { cli, out_ext: "csv".into(), extra_args: vec![], env: vec![] };
        let csv = convert(&spec, src, &dir).unwrap();
        assert_eq!(fs::read_to_string(&csv).unwrap(), crate::iwork::csv(&mine.tables[0].rows), "the CSV is the table, byte for byte");
        let xlsx = convert(&Spec { out_ext: "xlsx".into(), ..spec.clone() }, src, &dir).unwrap();
        // every value of the table, asked back from the workbook the way a formula would read it
        let wb0 = crate::numbers::read_workbook(src).unwrap();
        let wrong = cells_that_differ(&spec.cli, &wb0, &xlsx, &dir, true).unwrap();
        let checked: usize = wb0.tables[0].cells.iter().flatten().filter(|c| c.value != crate::numbers::Value::Empty).count();
        println!("{checked} cells compared, {} differ: {wrong:?}", wrong.len());
        assert!(wrong.is_empty(), "{wrong:?}");
        // and what the values do not say: formulas, formats and conditional styles are in the workbook
        let wb = crate::numbers::read_workbook(src).unwrap();
        let formulas: Vec<(usize, usize, String)> = wb.tables[0].cells.iter().enumerate().flat_map(|(r, row)| row.iter().enumerate().filter_map(move |(c, cell)| cell.formula.clone().map(|f| (r, c, f)))).collect();
        assert!(!formulas.is_empty(), "this file has a formula");
        for (r, c, f) in &formulas {
            let out = std::process::Command::new(&spec.cli).arg("cat").arg(&xlsx).arg("--range").arg(format!("{0}:{0}", crate::numbers::cell_name(*r, *c))).arg("--formulas").output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), f, "the formula travelled");
        }
        let info = std::process::Command::new(&spec.cli).arg("info").arg("--json").arg(&xlsx).output().unwrap();
        let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
        let conditional = info["sheets"][0]["conditionalFormats"].as_u64().unwrap_or(0);
        let wants = wb.tables[0].cond_sets.values().map(Vec::len).sum::<usize>();
        println!("conditional formats in the workbook: {conditional} (Numbers has {wants} rule(s))");
        assert!(wants == 0 || conditional > 0, "the conditional style travelled");
        let pdf = convert(&Spec { out_ext: "pdf".into(), ..spec }, src, &dir).unwrap();
        assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// With `VOIDFLARE_TEST_NUMBERS_CHARTS` (a `.numbers` with charts) and GridCraft: every chart arrives with the type
    /// and the number of series Numbers has, whichever way GridCraft first read the rows and columns.
    #[test]
    fn real_numbers_charts_arrive() {
        let (Some(path), Some(cli)) = (std::env::var_os("VOIDFLARE_TEST_NUMBERS_CHARTS"), GRIDCRAFT.find()) else { return };
        let src = Path::new(&path);
        let dir = job::scratch_dir().unwrap();
        let wb = crate::numbers::read_workbook(src).unwrap();
        let spec = Spec { cli: cli.clone(), out_ext: "xlsx".into(), extra_args: vec![], env: vec![] };
        let xlsx = convert(&spec, src, &dir).unwrap();
        let info = std::process::Command::new(&cli).arg("info").arg("--json").arg(&xlsx).output().unwrap();
        let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
        let (mut want, mut got_ok) = (0, 0);
        for t in &wb.tables {
            let sheet = info["sheets"].as_array().unwrap().iter().find(|s| s["name"].as_str() == Some(t.xl_name.as_str())).expect("the worksheet");
            let built = sheet["charts"].as_array().unwrap();
            assert_eq!(built.len(), t.charts.len(), "{}: charts", t.xl_name);
            for (k, ch) in t.charts.iter().enumerate() {
                want += 1;
                let n = built[k]["series"].as_u64().unwrap() as usize;
                println!("{} #{k}: {} {} series {n} (Numbers {})", t.xl_name, ch.kind, built[k]["kind"], ch.series.len());
                if n == ch.series.len() {
                    got_ok += 1;
                }
            }
        }
        assert!(want > 0);
        assert_eq!(want, got_ok, "series of every chart as in Numbers");
        let pdf = convert(&Spec { out_ext: "pdf".into(), ..spec }, src, &dir).unwrap();
        assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Every sheet reaches its own CSV, and an Italian CSV becomes numbers and dates (skipped without GridCraft).
    #[test]
    fn real_engine_sheets_and_italian_csv() {
        let Some(cli) = GRIDCRAFT.find() else {
            eprintln!("GridCraft not installed: skipped");
            return;
        };
        let dir = job::scratch_dir().unwrap();
        let book = dir.join("conti città.xlsx");
        two_sheet_xlsx(&book);
        let spec = Spec { cli: cli.clone(), out_ext: "csv".into(), extra_args: vec![], env: vec![] };
        let notes = std::cell::RefCell::new(Vec::new());
        let first = convert_noted(&spec, &book, &dir, &|n| notes.borrow_mut().push(n)).unwrap();
        assert_eq!(first.file_name().unwrap(), "conti città - Gennaio.csv");
        let second = dir.join("conti città - Vendite _Nord_.csv");
        assert_eq!(notes.borrow().as_slice(), ["sheet Gennaio → conti città - Gennaio.csv", "sheet Vendite \"Nord\" → conti città - Vendite _Nord_.csv"]);
        assert!(fs::read_to_string(&first).unwrap().starts_with("primo,12.5"), "{}", fs::read_to_string(&first).unwrap());
        assert!(fs::read_to_string(&second).unwrap().starts_with("secondo,7"));

        // Italian Excel: `;`, decimal comma, day-first date, a code, Windows-1252.
        let csv = dir.join("spese.csv");
        fs::write(&csv, b"voce;importo;data;codice\r\nCaff\xe8;1.234,5;05/11/2026;00123\r\n").unwrap();
        let xlsx = convert(&Spec { out_ext: "xlsx".into(), ..spec.clone() }, &csv, &dir).unwrap();
        let back = convert(&spec, &xlsx, &dir.join("back")).unwrap();
        let text = fs::read_to_string(&back).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert!(row.starts_with("Caffè,1234.5,"), "{text}");
        assert!(row.ends_with(",00123"), "the code kept its zeros: {text}");
        assert!(row.contains(",2026-11-05,"), "5 November, not 11 May: {text}");
        let pdf = convert(&Spec { out_ext: "pdf".into(), ..spec }, &csv, &dir).unwrap();
        assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
        let _ = fs::remove_dir_all(&dir);
    }
}
