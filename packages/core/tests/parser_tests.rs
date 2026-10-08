use exameow_core::parser::{extract_csv, extract_docx, extract_epub, extract_excel, extract_html, extract_odt, extract_pdf, extract_pptx, extract_txt, parse_file, FileFormat, ParserError};

#[test]
fn test_extract_txt() {
    let text = extract_txt("tests/fixtures/sample.txt").unwrap();
    assert!(text.contains("Machine Learning"));
    assert!(text.contains("Chapter 1"));
}

#[test]
fn test_extract_txt_not_found() {
    let result = extract_txt("tests/fixtures/nonexistent.txt");
    assert!(result.is_err());
}

#[test]
fn test_file_format_from_extension() {
    assert!(matches!(FileFormat::from_extension("doc.pdf"), Ok(FileFormat::Pdf)));
    assert!(matches!(FileFormat::from_extension("doc.docx"), Ok(FileFormat::Docx)));
    assert!(matches!(FileFormat::from_extension("doc.txt"), Ok(FileFormat::PlainText)));
    assert!(matches!(FileFormat::from_extension("main.py"), Ok(FileFormat::PlainText)));
    assert!(matches!(FileFormat::from_extension("README.md"), Ok(FileFormat::PlainText)));
    // unknown extensions fall back to PlainText (binary content is rejected at read time)
    assert!(matches!(FileFormat::from_extension("doc.png"), Ok(FileFormat::PlainText)));
    assert!(matches!(FileFormat::from_extension("Makefile"), Ok(FileFormat::PlainText)));
}

#[test]
fn test_read_gb18030_txt() {
    let path = std::env::temp_dir().join("exameow_test_gb18030.txt");
    let bytes: Vec<u8> = vec![0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4, b' ', b'o', b'k'];
    std::fs::write(&path, &bytes).unwrap();
    let text = extract_txt(path.to_str().unwrap()).unwrap();
    assert!(text.contains("中文测试"));
    assert!(text.contains("ok"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_read_binary_rejected() {
    let path = std::env::temp_dir().join("exameow_test_binary.bin");
    std::fs::write(&path, [0u8, 159, 146, 150, 0, 0, 12, 255]).unwrap();
    let result = extract_txt(path.to_str().unwrap());
    assert!(matches!(result, Err(ParserError::Unsupported(_))));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_read_utf8_bom_stripped() {
    let path = std::env::temp_dir().join("exameow_test_bom.txt");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("hello bom".as_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let text = extract_txt(path.to_str().unwrap()).unwrap();
    assert_eq!(text, "hello bom");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_csv_markdown_table() {
    let path = std::env::temp_dir().join("exameow_test.csv");
    std::fs::write(&path, "name,score\nAlice,95\nBob,87\n").unwrap();
    let text = extract_csv(path.to_str().unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "| name | score |");
    assert_eq!(lines[1], "| --- | --- |");
    assert_eq!(lines[2], "| Alice | 95 |");
    assert_eq!(lines[3], "| Bob | 87 |");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_csv_escapes_pipes_and_newlines() {
    let path = std::env::temp_dir().join("exameow_test_esc.csv");
    std::fs::write(&path, "a,b\n\"x|y\",\"line1\nline2\"\n").unwrap();
    let text = extract_csv(path.to_str().unwrap()).unwrap();
    assert!(text.contains("x\\|y"));
    assert!(text.contains("line1 line2"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_csv_empty_rejected() {
    let path = std::env::temp_dir().join("exameow_test_empty.csv");
    std::fs::write(&path, "\n\n").unwrap();
    assert!(extract_csv(path.to_str().unwrap()).is_err());
    let _ = std::fs::remove_file(&path);
}

fn write_min_xlsx(path: &std::path::Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o = SimpleFileOptions::default();
    z.start_file("[Content_Types].xml", o).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#).unwrap();
    z.start_file("_rels/.rels", o).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#).unwrap();
    z.start_file("xl/workbook.xml", o).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Scores" sheetId="1" r:id="rId1"/></sheets></workbook>"#).unwrap();
    z.start_file("xl/_rels/workbook.xml.rels", o).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#).unwrap();
    z.start_file("xl/worksheets/sheet1.xml", o).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Score</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Alice</t></is></c><c r="B2"><v>95</v></c></row></sheetData></worksheet>"#).unwrap();
    z.finish().unwrap();
}

#[test]
fn test_extract_excel_markdown_table() {
    let path = std::env::temp_dir().join("exameow_test.xlsx");
    write_min_xlsx(&path);
    let text = extract_excel(path.to_str().unwrap()).unwrap();
    assert!(text.contains("| Name | Score |"));
    assert!(text.contains("| --- | --- |"));
    assert!(text.contains("| Alice | 95 |"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_excel_invalid_rejected() {
    let path = std::env::temp_dir().join("exameow_test_bad.xlsx");
    std::fs::write(&path, "not a zip").unwrap();
    assert!(extract_excel(path.to_str().unwrap()).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_html_strips_tags() {
    let path = std::env::temp_dir().join("exameow_test.html");
    std::fs::write(&path, "<html><head><title>T</title><style>body{color:red}</style></head><body><script>var x=1;</script><h1>Chapter &amp; Intro</h1><p>Hello <b>world</b>&nbsp;&#20013;</p><!-- comment --></body></html>").unwrap();
    let text = extract_html(path.to_str().unwrap()).unwrap();
    assert!(text.contains("Chapter & Intro"));
    assert!(text.contains("Hello world \u{4e2d}"));
    assert!(!text.contains("var x"));
    assert!(!text.contains("color:red"));
    assert!(!text.contains("<"));
    assert!(!text.contains("comment"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_html_empty_rejected() {
    let path = std::env::temp_dir().join("exameow_test_empty.html");
    std::fs::write(&path, "<html><body><script>only()</script></body></html>").unwrap();
    assert!(extract_html(path.to_str().unwrap()).is_err());
    let _ = std::fs::remove_file(&path);
}

fn write_min_pptx(path: &std::path::Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o = SimpleFileOptions::default();
    let slide = |body: &str| format!(r#"<?xml version="1.0"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#);
    // write slide2 before slide1 to prove numeric ordering
    z.start_file("ppt/slides/slide2.xml", o).unwrap();
    z.write_all(slide("Second slide").as_bytes()).unwrap();
    z.start_file("ppt/slides/slide1.xml", o).unwrap();
    z.write_all(slide("First slide").as_bytes()).unwrap();
    z.finish().unwrap();
}

#[test]
fn test_extract_pptx_slides_in_order() {
    let path = std::env::temp_dir().join("exameow_test.pptx");
    write_min_pptx(&path);
    let text = extract_pptx(path.to_str().unwrap()).unwrap();
    let first = text.find("First slide").unwrap();
    let second = text.find("Second slide").unwrap();
    assert!(first < second);
    assert!(text.contains("### Slide 1"));
    assert!(text.contains("### Slide 2"));
    let _ = std::fs::remove_file(&path);
}

fn write_min_odt(path: &std::path::Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o = SimpleFileOptions::default();
    z.start_file("content.xml", o).unwrap();
    z.write_all(br#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text><text:h>Title Here</text:h><text:p>First paragraph.</text:p><text:p>Second <text:span>styled</text:span> paragraph.</text:p></office:text></office:body></office:document-content>"#).unwrap();
    z.finish().unwrap();
}

fn write_min_epub(path: &std::path::Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o = SimpleFileOptions::default();
    z.start_file("META-INF/container.xml", o).unwrap();
    z.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
    z.start_file("OEBPS/content.opf", o).unwrap();
    z.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#).unwrap();
    z.start_file("OEBPS/ch1.xhtml", o).unwrap();
    z.write_all(br#"<html><body><p>Alpha chapter</p></body></html>"#).unwrap();
    z.start_file("OEBPS/ch2.xhtml", o).unwrap();
    z.write_all(br#"<html><body><p>Beta chapter</p></body></html>"#).unwrap();
    z.finish().unwrap();
}

#[test]
fn test_extract_epub_spine_order() {
    let path = std::env::temp_dir().join("exameow_test.epub");
    write_min_epub(&path);
    let text = extract_epub(path.to_str().unwrap()).unwrap();
    let beta = text.find("Beta chapter").unwrap();
    let alpha = text.find("Alpha chapter").unwrap();
    assert!(beta < alpha, "spine order (c2 before c1) must be respected");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_extract_odt_paragraphs() {
    let path = std::env::temp_dir().join("exameow_test.odt");
    write_min_odt(&path);
    let text = extract_odt(path.to_str().unwrap()).unwrap();
    assert!(text.contains("Title Here"));
    assert!(text.contains("First paragraph."));
    assert!(text.contains("Second styled paragraph."));
    let title_pos = text.find("Title Here").unwrap();
    let first_pos = text.find("First paragraph.").unwrap();
    assert!(title_pos < first_pos);
    let _ = std::fs::remove_file(&path);
}

// ───────── XML entities (quick-xml reports them as separate events) ─────────

fn zip_of(path: &std::path::Path, entries: &[(&str, &str)]) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in entries {
        z.start_file(*name, SimpleFileOptions::default()).unwrap();
        z.write_all(body.as_bytes()).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn test_docx_decodes_entities_and_arabic_runs() {
    let path = std::env::temp_dir().join("exameow_entities.docx");
    zip_of(&path, &[("word/document.xml", r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t xml:space="preserve">Tom &amp; Jerry &lt;b&gt; &quot;quoted&quot; &#1575;&#x644;</w:t></w:r></w:p>
        <w:p><w:r><w:t>مقدمة في قواعد البيانات</w:t></w:r></w:p>
        <w:p><w:r><w:t>   </w:t></w:r></w:p>
        <w:p><w:r><w:t>second</w:t></w:r><w:r><w:t>third</w:t></w:r></w:p></w:body></w:document>"#)]);
    let text = extract_docx(path.to_str().unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, vec!["Tom & Jerry <b> \"quoted\" ال", "مقدمة في قواعد البيانات", "second", "third"]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_pptx_and_odt_decode_entities() {
    let pptx = std::env::temp_dir().join("exameow_entities.pptx");
    zip_of(&pptx, &[("ppt/slides/slide1.xml", r#"<?xml version="1.0"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Q&amp;A: 2 &lt; 3</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#)]);
    assert!(extract_pptx(pptx.to_str().unwrap()).unwrap().contains("Q&A: 2 < 3"));
    let odt = std::env::temp_dir().join("exameow_entities.odt");
    zip_of(&odt, &[("content.xml", r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text><text:p>R&amp;D &#8212; 5 &gt; 4 &apos;ok&apos;</text:p></office:text></office:body></office:document-content>"#)]);
    assert!(extract_odt(odt.to_str().unwrap()).unwrap().contains("R&D — 5 > 4 'ok'"));
    let _ = std::fs::remove_file(&pptx);
    let _ = std::fs::remove_file(&odt);
}

// ───────── PDF ─────────

#[test]
fn test_arabic_pdf_is_extracted_in_reading_order() {
    // printed by Chromium from an Arabic page: shaped glyphs, right-to-left lines stored in visual order
    let text = extract_pdf("tests/fixtures/arabic.pdf").unwrap();
    for expected in ["مقدمة في قواعد البيانات", "قاعدة البيانات هي", "نظام", "المفتاح", "القيم الفارغة"] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(!text.chars().any(|c| matches!(c as u32, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)), "presentation forms must be normalised:\n{text}");
    let title = text.find("مقدمة في قواعد البيانات").unwrap();
    let body = text.find("قاعدة البيانات هي").unwrap();
    assert!(title < body, "lines keep their order");
}

#[test]
fn test_parse_file_dispatches_pdf_through_the_arabic_fix() {
    let text = parse_file("tests/fixtures/arabic.pdf").unwrap();
    assert!(text.contains("قواعد البيانات"));
}

/// A PDF whose catalog holds an array nested `depth` levels deep. Robustness regression test: hostile nesting must
/// come back as an error, never as a stack overflow that aborts the whole server process (lopdf is upgraded past
/// RUSTSEC-2026-0187, which is about exactly that class of input).
fn nested_pdf(depth: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R /X ".to_vec();
    v.extend(std::iter::repeat(b'[').take(depth));
    v.extend(std::iter::repeat(b']').take(depth));
    v.extend_from_slice(b" >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R /Size 3 >>\n%%EOF\n");
    v
}

#[test]
fn test_deeply_nested_pdf_is_rejected_without_crashing_the_process() {
    for depth in [50_000usize, 1_000_000] {
        let path = std::env::temp_dir().join(format!("exameow_nested_{depth}.pdf"));
        std::fs::write(&path, nested_pdf(depth)).unwrap();
        // the assertion is that this returns at all (a stack overflow would kill the test binary)
        let r = extract_pdf(path.to_str().unwrap());
        assert!(r.is_err(), "no text can come out of a PDF without pages");
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn test_garbage_pdfs_are_errors_not_panics() {
    for (i, bytes) in [b"%PDF-1.4".to_vec(), b"not a pdf at all".to_vec(), vec![0u8; 4096], b"%PDF-1.7\n1 0 obj\n<<".to_vec()].into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("exameow_garbage_{i}.pdf"));
        std::fs::write(&path, bytes).unwrap();
        assert!(extract_pdf(path.to_str().unwrap()).is_err(), "case {i}");
        let _ = std::fs::remove_file(&path);
    }
}
