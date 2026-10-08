//! Generic table export (results, statistics): multi-sheet XLSX and CSV from rows of text/number cells.
//!
//! Like the question export this is hand-written ZIP+XML (no spreadsheet library). Text is stored as
//! shared strings, so a spreadsheet application never interprets a cell as a formula; the CSV writer
//! neutralises leading formula characters instead, because CSV has no cell types.

use super::xlsx::{col_letter, escape_xml};
use crate::error::CoreError;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use zip::write::FileOptions;
use zip::CompressionMethod;

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(String),
    Num(f64),
}

impl From<&str> for Cell {
    fn from(s: &str) -> Cell {
        Cell::Text(s.to_string())
    }
}
impl From<String> for Cell {
    fn from(s: String) -> Cell {
        Cell::Text(s)
    }
}
impl From<f64> for Cell {
    fn from(n: f64) -> Cell {
        Cell::Num(n)
    }
}
impl From<i64> for Cell {
    fn from(n: i64) -> Cell {
        Cell::Num(n as f64)
    }
}

#[derive(Clone, Debug)]
pub struct Sheet {
    pub name: String,
    /// The first row is the header (bold, frozen).
    pub rows: Vec<Vec<Cell>>,
    /// Right-to-left sheet (Arabic).
    pub rtl: bool,
}

/// Characters that are not allowed in XML 1.0 are dropped (a stray control character would corrupt the file).
fn clean_xml(s: &str) -> String {
    s.chars().filter(|c| matches!(*c, '\t' | '\n' | '\r') || (*c as u32) >= 0x20 && !matches!(*c as u32, 0xFFFE | 0xFFFF)).collect()
}

/// Excel sheet names: ≤31 chars, none of `[]:*?/\`, not empty, unique (case-insensitive).
fn sheet_names(sheets: &[Sheet]) -> Vec<String> {
    let mut used: Vec<String> = vec![];
    for (i, s) in sheets.iter().enumerate() {
        let mut n: String = s.name.chars().filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\') && !c.is_control()).collect();
        n = n.trim().trim_matches('\'').chars().take(31).collect();
        if n.is_empty() {
            n = format!("Sheet{}", i + 1);
        }
        let base = n.clone();
        let mut k = 2;
        while used.iter().any(|u| u.to_lowercase() == n.to_lowercase()) {
            let suffix = format!(" ({k})");
            n = format!("{}{}", base.chars().take(31 - suffix.chars().count()).collect::<String>(), suffix);
            k += 1;
        }
        used.push(n);
    }
    used
}

pub fn export_tables_xlsx(sheets: &[Sheet]) -> Result<Vec<u8>, CoreError> {
    if sheets.is_empty() {
        return Err(CoreError::Export("no sheets".into()));
    }
    let names = sheet_names(sheets);
    let mut strings: Vec<String> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut intern = |s: &str| -> usize {
        let c = clean_xml(s);
        if let Some(i) = index.get(&c) {
            return *i;
        }
        let i = strings.len();
        strings.push(c.clone());
        index.insert(c, i);
        i
    };

    let mut sheet_xml: Vec<String> = vec![];
    for sh in sheets {
        let cols = sh.rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
        // header row frozen; RTL flips the sheet direction for Arabic
        xml.push_str(&format!(
            r#"<sheetViews><sheetView workbookViewId="0"{}><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#,
            if sh.rtl { r#" rightToLeft="1""# } else { "" }
        ));
        xml.push_str(&format!(r#"<cols><col min="1" max="{cols}" width="20" customWidth="1"/></cols><sheetData>"#));
        for (r, row) in sh.rows.iter().enumerate() {
            xml.push_str(&format!(r#"<row r="{}">"#, r + 1));
            for (c, cell) in row.iter().enumerate() {
                let at = format!("{}{}", col_letter(c), r + 1);
                let style = if r == 0 { r#" s="1""# } else { "" };
                match cell {
                    Cell::Text(t) if t.is_empty() => {}
                    Cell::Text(t) => xml.push_str(&format!(r#"<c r="{at}"{style} t="s"><v>{}</v></c>"#, intern(t))),
                    Cell::Num(n) if n.is_finite() => xml.push_str(&format!(r#"<c r="{at}"{style}><v>{n}</v></c>"#)),
                    Cell::Num(_) => {}
                }
            }
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData></worksheet>");
        sheet_xml.push(xml);
    }

    let mut sst = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{n}" uniqueCount="{n}">"#,
        n = strings.len()
    );
    for s in &strings {
        sst.push_str(&format!(r#"<si><t xml:space="preserve">{}</t></si>"#, escape_xml(s)));
    }
    sst.push_str("</sst>");

    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs>
</styleSheet>"#;

    let mut content_types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
    );
    let mut workbook = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
    );
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (i, name) in names.iter().enumerate() {
        let n = i + 1;
        content_types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        workbook.push_str(&format!(r#"<sheet name="{}" sheetId="{n}" r:id="rId{n}"/>"#, escape_xml(name)));
        rels.push_str(&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
    }
    let k = names.len();
    rels.push_str(&format!(
        r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
        k + 1,
        k + 2
    ));
    content_types.push_str("</Types>");
    workbook.push_str("</sheets></workbook>");
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = FileOptions::<()>::default().compression_method(CompressionMethod::Deflated);
    let mut put = |path: &str, body: &str| -> Result<(), CoreError> {
        zip.start_file(path, opts).map_err(|e| CoreError::Export(format!("zip error: {e}")))?;
        zip.write_all(body.as_bytes()).map_err(|e| CoreError::Export(format!("zip write error: {e}")))
    };
    put("[Content_Types].xml", &content_types)?;
    put("_rels/.rels", root_rels)?;
    put("xl/workbook.xml", &workbook)?;
    put("xl/_rels/workbook.xml.rels", &rels)?;
    for (i, xml) in sheet_xml.iter().enumerate() {
        put(&format!("xl/worksheets/sheet{}.xml", i + 1), xml)?;
    }
    put("xl/sharedStrings.xml", &sst)?;
    put("xl/styles.xml", styles)?;
    Ok(zip.finish().map_err(|e| CoreError::Export(format!("zip finalize error: {e}")))?.into_inner())
}

/// A text cell that a spreadsheet could read as a formula (`=`, `+`, `-`, `@`, tab, CR) is prefixed with `'`.
fn neutralise(t: &str) -> String {
    if t.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{t}")
    } else {
        t.to_string()
    }
}

/// UTF-8 CSV with a BOM (so Excel opens Arabic correctly); numbers are written bare, text is neutralised.
pub fn export_table_csv(rows: &[Vec<Cell>]) -> Result<Vec<u8>, CoreError> {
    let mut out = vec![0xEF, 0xBB, 0xBF];
    {
        let mut w = csv::WriterBuilder::new().flexible(true).from_writer(&mut out);
        for row in rows {
            let rec: Vec<String> = row
                .iter()
                .map(|c| match c {
                    Cell::Text(t) => neutralise(&clean_xml(t)),
                    Cell::Num(n) if n.is_finite() => n.to_string(),
                    Cell::Num(_) => String::new(),
                })
                .collect();
            w.write_record(&rec).map_err(|e| CoreError::Export(format!("write error: {e}")))?;
        }
        w.flush().map_err(|e| CoreError::Export(format!("flush error: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn unzip(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut f = z.by_name(name).unwrap_or_else(|_| panic!("{name} missing"));
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    }

    fn sheets() -> Vec<Sheet> {
        vec![
            Sheet {
                name: "النتائج".into(),
                rtl: true,
                rows: vec![
                    vec!["الاسم".into(), "الدرجة".into(), "ملاحظة".into()],
                    vec!["سارة <b>&\"'".into(), Cell::Num(8.5), "=HYPERLINK(\"http://x\")".into()],
                    vec!["عمر".into(), Cell::Num(f64::NAN), "  مسافات  \n سطر".into()],
                ],
            },
            Sheet { name: "Bad:/name?[x]".into(), rtl: false, rows: vec![vec!["a".into()]] },
            Sheet { name: "bad name x".into(), rtl: false, rows: vec![vec![Cell::Num(1.0)]] },
        ]
    }

    #[test]
    fn xlsx_is_a_valid_multi_sheet_workbook_with_typed_cells() {
        let b = export_tables_xlsx(&sheets()).unwrap();
        assert_eq!(&b[..2], b"PK");
        let wb = unzip(&b, "xl/workbook.xml");
        assert!(wb.contains(r#"name="النتائج""#), "{wb}");
        assert!(wb.contains(r#"name="Badnamex""#) && wb.contains(r#"name="bad name x""#), "forbidden sheet-name characters removed: {wb}");
        assert!(!wb.contains(r#"name="Bad:"#));
        let s1 = unzip(&b, "xl/worksheets/sheet1.xml");
        assert!(s1.contains(r#"rightToLeft="1""#) && s1.contains(r#"state="frozen""#), "RTL + frozen header");
        assert!(s1.contains(r#"<c r="B2"><v>8.5</v></c>"#), "numbers are numeric cells: {s1}");
        assert!(!s1.contains(r#"r="B3""#), "NaN becomes an empty cell");
        assert!(s1.contains(r#"<c r="A1" s="1" t="s">"#), "header cells are bold");
        assert!(unzip(&b, "xl/worksheets/sheet2.xml").contains("sheetData") && !unzip(&b, "xl/worksheets/sheet2.xml").contains("rightToLeft"));
        let sst = unzip(&b, "xl/sharedStrings.xml");
        assert!(sst.contains("سارة &lt;b&gt;&amp;&quot;&apos;"), "XML special characters are escaped: {sst}");
        assert!(sst.contains("=HYPERLINK") && s1.contains(r#"t="s""#), "a formula-looking string stays a plain string cell (never a formula)");
        assert!(sst.contains(r#"xml:space="preserve">  مسافات  "#), "whitespace is preserved");
        assert!(!s1.contains("<f>"), "no formula cells at all");
    }

    #[test]
    fn control_characters_and_duplicate_or_empty_sheet_names_are_handled() {
        let s = vec![
            Sheet { name: "Data".into(), rtl: false, rows: vec![vec!["a\u{0}b\u{8}c\u{1F}d\te".into()]] },
            Sheet { name: "data".into(), rtl: false, rows: vec![vec!["x".into()]] },
            Sheet { name: "///".into(), rtl: false, rows: vec![vec!["y".into()]] },
            Sheet { name: "ع".repeat(40), rtl: true, rows: vec![vec!["z".into()]] },
        ];
        let b = export_tables_xlsx(&s).unwrap();
        let sst = unzip(&b, "xl/sharedStrings.xml");
        assert!(sst.contains("abcd\te"), "invalid XML control characters stripped, tab kept: {sst:?}");
        let wb = unzip(&b, "xl/workbook.xml");
        assert!(wb.contains(r#"name="Data""#) && wb.contains(r#"name="data (2)""#) && wb.contains(r#"name="Sheet3""#), "{wb}");
        assert!(wb.contains(&format!(r#"name="{}""#, "ع".repeat(31))), "long names cut to 31 characters");
        assert!(export_tables_xlsx(&[]).is_err());
        let wide = Sheet { name: "w".into(), rtl: false, rows: vec![(0..30).map(|i| Cell::Num(i as f64)).collect()] };
        assert!(unzip(&export_tables_xlsx(&[wide]).unwrap(), "xl/worksheets/sheet1.xml").contains(r#"r="AD1""#), "columns past Z get two-letter references");
    }

    #[test]
    fn csv_has_a_bom_quotes_properly_and_neutralises_formulas() {
        let rows = vec![
            vec![Cell::from("الاسم"), "الدرجة".into(), "ملاحظة".into()],
            vec!["=cmd|' /C calc'!A0".into(), Cell::Num(-5.0), "+1".into()],
            vec!["a,b \"q\"\nline".into(), Cell::Num(8.25), "@SUM(1)".into()],
            vec!["-1+1".into(), Cell::Num(f64::INFINITY), "\t tab".into()],
            vec!["عادي".into(), Cell::Num(3.0), "  ".into()],
        ];
        let b = export_table_csv(&rows).unwrap();
        assert_eq!(&b[..3], &[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8(b[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "الاسم,الدرجة,ملاحظة");
        assert_eq!(lines[1], "'=cmd|' /C calc'!A0,-5,'+1", "text is neutralised but a real negative number is untouched");
        assert!(text.contains("\"a,b \"\"q\"\"\nline\",8.25,'@SUM(1)"), "{text}");
        assert!(text.contains("'-1+1,,'\t tab"), "infinity becomes an empty cell: {text}");
        let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(&b[3..]);
        assert_eq!(rdr.records().count(), 5, "round-trips as 5 records even with an embedded newline");
    }
}
