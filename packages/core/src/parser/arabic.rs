//! Arabic text coming out of PDFs.
//!
//! PDF producers store glyphs in *visual* order and, for Arabic, as shaped presentation forms (initial / medial /
//! final variants in U+FB50–FDFF and U+FE70–FEFF). Extractors that follow the content stream therefore return each
//! right-to-left line reversed (words and letters), in presentation forms — useless to a language model and to a
//! student reading the question. This module converts that back to normal logical Arabic:
//!
//! 1. presentation forms → base letters (NFKC), which also splits ligatures such as U+FEFB into "لا";
//! 2. decides whether the text is reversed, by counting words that begin with the article «ال» (normal order) against
//!    words that end with «لا» (what «ال…» becomes when reversed); with too little evidence the presence of
//!    presentation forms decides, since they only occur in shaped, visual-order text;
//! 3. reverses every mostly-Arabic line, then puts numbers and Latin words (which a whole-line reversal flips) back.
//!
//! Text that is already in logical order, and text without Arabic, passes through unchanged. Keep this file in sync
//! with `frontend/src/utils/arabicPdf.ts` and `workers/src/arabicPdf.ts` (same algorithm, same tests' expectations).

use unicode_normalization::UnicodeNormalization;

fn is_presentation_form(c: char) -> bool {
    matches!(c as u32, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}

fn is_arabic_letter(c: char) -> bool {
    let cp = c as u32;
    let in_block = matches!(cp, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08C9) || is_presentation_form(c);
    in_block && !matches!(cp, 0x064B..=0x065F | 0x0670)
}

/// Whether the text contains any Arabic letters (base or presentation forms).
pub fn has_arabic(s: &str) -> bool {
    s.chars().any(is_arabic_letter)
}

/// Digits and Latin letters keep their left-to-right order inside a right-to-left line.
fn is_ltr_run_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c as u32, 0x0660..=0x0669 | 0x06F0..=0x06F9)
}

/// Presentation forms → base letters. A ligature glyph (one char standing for two or more letters, e.g. «ﻻ» = «لا»)
/// expands in logical order; when the text is about to be reversed line-wise (`for_visual`) the expansion is put in
/// reverse so that the reversal restores it.
fn to_base_letters(s: &str, for_visual: bool) -> String {
    s.chars()
        .flat_map(|c| {
            if !is_presentation_form(c) {
                return vec![c];
            }
            let mut v: Vec<char> = c.nfkc().collect();
            if for_visual && v.len() > 1 {
                v.reverse();
            }
            v
        })
        .collect()
}

/// Separators that stay inside one number or Latin token when flanked by letters/digits (3.14, 12:30, 2024/05/01, a-b).
fn is_joiner(c: char) -> bool {
    matches!(c, '.' | ',' | ':' | '/' | '-' | '%' | '_')
}

/// Reverses a visual-order line to logical order, restoring the order of digit/Latin runs.
fn reverse_line(line: &str) -> String {
    let rev: Vec<char> = line.chars().rev().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < rev.len() {
        if is_ltr_run_char(rev[i]) {
            let start = i;
            while i < rev.len() && (is_ltr_run_char(rev[i]) || (is_joiner(rev[i]) && i + 1 < rev.len() && is_ltr_run_char(rev[i + 1]))) {
                i += 1;
            }
            out.extend(rev[start..i].iter().rev());
        } else {
            out.push(rev[i]);
            i += 1;
        }
    }
    out
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x064B..=0x065F | 0x0670)
}

/// Extractors position every glyph separately, so lines come back padded with doubled spaces and a few spurious gaps.
/// Two repairs that cannot be wrong: one space where there were several, and no space before a vowel mark or between a
/// lone alef and the lam that follows it (the article «ال» is never written apart).
fn tidy_line(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for tok in s.split(' ').filter(|t| !t.is_empty()) {
        match out.last_mut() {
            Some(prev) if tok.chars().next().map_or(false, is_mark) => prev.push_str(tok),
            Some(prev) if prev == "ا" && tok.starts_with('ل') => prev.push_str(tok),
            _ => out.push(tok.to_string()),
        }
    }
    let lead = s.len() - s.trim_start_matches(' ').len();
    format!("{}{}", " ".repeat(lead.min(1)), out.join(" "))
}

fn arabic_share(line: &str) -> f64 {
    let (mut arabic, mut letters) = (0usize, 0usize);
    for c in line.chars() {
        if is_arabic_letter(c) {
            arabic += 1;
            letters += 1;
        } else if c.is_alphabetic() {
            letters += 1;
        }
    }
    if letters == 0 { 0.0 } else { arabic as f64 / letters as f64 }
}

/// Counts of words starting with «ال» (incl. one-letter prefixes و ب ف ك ل) vs. words ending with the reversed forms.
fn article_evidence(text: &str) -> (usize, usize) {
    let (mut forward, mut reversed) = (0, 0);
    for w in text.split(|c: char| !c.is_alphabetic() && !matches!(c as u32, 0x064B..=0x065F | 0x0670)) {
        let letters: Vec<char> = w.chars().filter(|c| is_arabic_letter(*c)).collect();
        if letters.len() < 4 {
            continue;
        }
        let s = |a: &[char]| letters.starts_with(a);
        let e = |a: &[char]| letters.ends_with(a);
        if s(&['ا', 'ل']) || (matches!(letters[0], 'و' | 'ب' | 'ف' | 'ك' | 'ل') && letters[1..].starts_with(&['ا', 'ل'])) || s(&['ل', 'ل']) {
            forward += 1;
        }
        if e(&['ل', 'ا']) || (matches!(letters[letters.len() - 1], 'و' | 'ب' | 'ف' | 'ك' | 'ل') && letters[..letters.len() - 1].ends_with(&['ل', 'ا'])) || e(&['ل', 'ل']) {
            reversed += 1;
        }
    }
    (forward, reversed)
}

/// Converts Arabic text extracted from a PDF (visual order, presentation forms) to logical order.
pub fn fix_visual_order(text: &str) -> String {
    if !has_arabic(text) {
        return text.to_string();
    }
    let had_presentation_forms = text.chars().any(is_presentation_form);
    let logical = to_base_letters(text, false);
    let (forward, reversed) = article_evidence(&logical);
    let evidence = forward + reversed;
    let is_reversed = if evidence >= 3 { reversed > forward } else { had_presentation_forms };
    if !is_reversed {
        return logical;
    }
    to_base_letters(text, true).split('\n').map(|l| if arabic_share(l) >= 0.5 { tidy_line(&reverse_line(l)) } else { l.to_string() }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // lines exactly as pdf-extract returns them for a Chromium-printed Arabic page
    const TITLE: &str = "تﺎﻧﺎﻴﺒﻟا ﺪﻋاﻮﻗ ﻲﻓ ﺔﻣﺪﻘﻣ";

    #[test]
    fn a_shaped_visual_order_line_becomes_normal_arabic() {
        assert_eq!(fix_visual_order(TITLE), "مقدمة في قواعد البيانات");
    }

    #[test]
    fn numbers_and_latin_words_keep_their_order() {
        // visual order of the logical "سنة 2024 و SQL"  (RTL: rightmost is first)
        let visual = "SQL ﻭ 2024 ﺔﻨﺳ";
        assert_eq!(fix_visual_order(visual), "سنة 2024 و SQL");
        assert_eq!(fix_visual_order("ﻲﻓ 3.14 ﻢﻗﺭ"), "رقم 3.14 في");
    }

    #[test]
    fn text_already_in_logical_order_is_left_alone() {
        let logical = "نظام إدارة قاعدة البيانات يتيح الإنشاء والتعديل والاستعلام. المفتاح الأساسي يميز كل سجل في الجدول.";
        assert_eq!(fix_visual_order(logical), logical);
    }

    #[test]
    fn presentation_forms_are_always_normalised_even_without_evidence_of_reversal() {
        // logical order but shaped glyphs: only the forms change
        assert_eq!(to_base_letters("ﻣﺮﺣﺒﺎ", false), "مرحبا");
        assert_eq!(to_base_letters("ﻻ", false), "لا", "the lam-alef ligature splits in logical order");
        assert_eq!(to_base_letters("ﻻ", true), "ال", "…and in reverse when the line is about to be reversed");
        assert_eq!(to_base_letters("abc ١٢٣", false), "abc ١٢٣");
    }

    #[test]
    fn non_arabic_text_passes_through_byte_for_byte() {
        for s in ["Plain English text.\nSecond line 123", "", "你好，世界", "Ünïcödé — ok"] {
            assert_eq!(fix_visual_order(s), s);
        }
    }

    #[test]
    fn mixed_documents_only_touch_their_arabic_lines() {
        let doc = format!("Chapter 1: Databases\n{TITLE}\nA table has rows and columns.");
        assert_eq!(fix_visual_order(&doc), "Chapter 1: Databases\nمقدمة في قواعد البيانات\nA table has rows and columns.");
    }

    #[test]
    fn evidence_of_reversal_comes_from_the_article() {
        // «الكتاب الجديد» reversed letter by letter: «بادجلا باتكلا» in visual order
        let (fwd, rev) = article_evidence("بادجلا باتكلا ةرادإ");
        assert!(rev > fwd, "{fwd} {rev}");
        let (fwd, rev) = article_evidence("الكتاب الجديد والمكتبة");
        assert!(fwd > rev, "{fwd} {rev}");
    }

    #[test]
    fn a_whole_page_of_reversed_text_is_restored_line_by_line() {
        let visual = "ءﺎﺸﻧﻹا  ﺢﻴﺘﻳ تﺎﻧﺎﻴﺒﻟا ةﺪﻋﺎﻗ مﺎﻈﻧ\nلوﺪﺠﻟا ﻲﻓ ﻞﺠﺳ ﻞﻛ ﺰﻴﻤﻳ ﻲﺳﺎﺳﻷا حﺎﺘﻔﻤﻟا";
        let fixed = fix_visual_order(visual);
        assert!(fixed.contains("نظام قاعدة البيانات يتيح الإنشاء"), "{fixed}");
        assert!(fixed.contains("المفتاح الأساسي يميز كل سجل في الجدول"), "{fixed}");
    }

    #[test]
    fn spurious_gaps_are_repaired_but_real_spaces_stay() {
        assert_eq!(tidy_line("ا لإنشاء  و التعديل"), "الإنشاء و التعديل", "lone alef + lam, doubled space");
        assert_eq!(tidy_line("إلكترونيا ً ."), "إلكترونياً .", "a vowel mark never stands alone");
        assert_eq!(tidy_line("هذا ا لكتاب"), "هذا الكتاب");
        assert_eq!(tidy_line("ت ُخ ز ن"), "تُخ ز ن", "a mark glued to the next letter still belongs to the previous one");
        assert_eq!(tidy_line("كل  كتاب"), "كل كتاب");
        assert_eq!(tidy_line("ا بحث"), "ا بحث", "only the article pattern is merged");
    }

    #[test]
    fn marks_stay_attached_to_their_letters() {
        // fathatan after the alef in logical order → before it in the reversed (visual) stream
        let visual = "ﺎًﻴﻧوﺮﺘﻜﻟإ";
        let fixed = fix_visual_order(&format!("{visual} {TITLE}"));
        assert!(fixed.contains("إلكترونيًا"), "{fixed}"); // tanwin sits on the letter before the alef
    }
}
