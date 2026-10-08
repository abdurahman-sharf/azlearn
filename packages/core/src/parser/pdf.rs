use super::arabic::{fix_visual_order, has_arabic};
use super::ParserError;
use lopdf::Document;
use unicode_normalization::UnicodeNormalization;

// macOS Quartz 生成的 PDF 常把汉字映射到 Kangxi 部首/CJK 兼容字符码点
//（如 ⼈⼤⽹），视觉相同但码点不同，按 NFKC 归一化回标准汉字
fn normalize_compat_chars(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let cp = c as u32;
            if (0x2E80..=0x2EFF).contains(&cp)
                || (0x2F00..=0x2FDF).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
            {
                c.nfkc().collect::<Vec<char>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

pub fn extract_pdf(path: &str) -> Result<String, ParserError> {
    let primary = extract_via_lopdf(path).ok().filter(|t| !t.trim().is_empty());
    let text = match primary {
        // Arabic: lopdf returns one word per line with unmapped glyphs for some fonts; pdf-extract keeps whole lines
        // in a form `fix_visual_order` can restore, so it is preferred whenever it also finds Arabic.
        Some(t) if has_arabic(&t) => match extract_via_pdf_extract(path) {
            Ok(p) if has_arabic(&p) && !p.trim().is_empty() => p,
            _ => t,
        },
        Some(t) => t,
        None => extract_via_pdf_extract(path)?,
    };
    Ok(normalize_compat_chars(&fix_visual_order(&text)))
}

fn extract_via_lopdf(path: &str) -> Result<String, ParserError> {
    let doc = Document::load(path)
        .map_err(|e| ParserError::Parse(format!("pdf load error: {e}")))?;

    let mut texts = Vec::new();
    let total = doc.get_pages().len();

    for page_num in 1..=total as u32 {
        match doc.extract_text(&[page_num]) {
            Ok(text) => {
                let cleaned = text.trim().to_string();
                if !cleaned.is_empty() {
                    texts.push(cleaned);
                }
            }
            Err(e) => {
                eprintln!("[Exameow] lopdf page {}/{} failed: {e}", page_num, total);
            }
        }
    }

    let result = texts.join("\n");
    if result.trim().is_empty() {
        return Err(ParserError::Parse("lopdf: no extractable text".to_string()));
    }
    Ok(result)
}

fn extract_via_pdf_extract(path: &str) -> Result<String, ParserError> {
    let data = std::fs::read(path)
        .map_err(|e| ParserError::Io(e))?;
    pdf_extract::extract_text_from_mem(&data)
        .map_err(|e| ParserError::Parse(format!("pdf-extract error: {e}")))
}
