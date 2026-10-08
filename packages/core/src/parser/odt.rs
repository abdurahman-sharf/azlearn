use super::ParserError;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;

pub fn extract_odt(path: &str) -> Result<String, ParserError> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ParserError::Parse(format!("invalid odt zip: {e}")))?;

    let mut xml = String::new();
    archive
        .by_name("content.xml")
        .map_err(|e| ParserError::Parse(format!("missing content.xml: {e}")))?
        .read_to_string(&mut xml)?;

    let mut reader = Reader::from_str(&xml);
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(ref t)) => {
                let s = t.decode().unwrap_or_default();
                if !s.trim().is_empty() {
                    out.push_str(&s);
                }
            }
            Ok(Event::GeneralRef(ref r)) => out.push_str(&super::xml_text::resolve_ref(r)),
            Ok(Event::End(ref e)) => {
                if matches!(e.local_name().as_ref(), b"p" | b"h") && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParserError::Parse(format!("xml error: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    if out.trim().is_empty() {
        return Err(ParserError::Parse("no text found in odt".to_string()));
    }
    Ok(out.trim().to_string())
}
