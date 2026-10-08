use super::ParserError;
use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;

/// The character an entity/character reference stands for (`&amp;` → `&`, `&#1575;` → `ا`); unknown names give "".
/// quick-xml 0.38+ reports references as their own events instead of unescaping them inside text.
pub(crate) fn resolve_ref(r: &BytesRef<'_>) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return c.to_string();
    }
    let name = r.decode().unwrap_or_default();
    quick_xml::escape::resolve_predefined_entity(&name).unwrap_or("").to_string()
}

/// Text of every `<target>` element, with entities resolved (`<w:t>A &amp; B</w:t>` → `A & B`).
pub(crate) fn collect_texts(xml: &str, target: &[u8]) -> Result<Vec<String>, ParserError> {
    let mut reader = Reader::from_str(xml);
    let mut texts = Vec::new();
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.local_name().as_ref() == target => current = Some(String::new()),
            Ok(Event::Text(ref t)) => {
                if let Some(s) = current.as_mut() {
                    s.push_str(&t.decode().unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(ref r)) => {
                if let Some(s) = current.as_mut() {
                    s.push_str(&resolve_ref(r));
                }
            }
            Ok(Event::End(ref e)) if e.local_name().as_ref() == target => {
                if let Some(s) = current.take() {
                    if !s.trim().is_empty() {
                        texts.push(s.trim().to_string());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParserError::Parse(format!("xml error: {e}"))),
            _ => {}
        }
    }
    Ok(texts)
}
