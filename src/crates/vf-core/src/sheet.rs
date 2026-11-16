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
