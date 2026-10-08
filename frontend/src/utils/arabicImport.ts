/**
 * Arabic vocabulary for importing questions from spreadsheets: column headers, question-type labels, difficulty words
 * and Arabic-letter answers (أ ب ج د). Pure functions, no dependencies, so `importParser` can use them and plain
 * `node` can test them.
 *
 * Headers are matched *exactly* after folding (not with `includes`): "نوع السؤال" contains "السؤال", so substring
 * matching would hand the type column to the stem.
 */

/** Invisible marks spreadsheets and RTL text carry: BOM, zero-width/bidi controls, diacritics, tatweel. */
const MARKS = /[\uFEFF\u200B-\u200F\u202A-\u202E\u2066-\u2069\u064B-\u065F\u0670\u0640]/g

/** Comparison form: no diacritics/tatweel/BOM/bidi marks/spaces/`:*`, أإآٱ→ا, ى→ي, ة→ه, lower-case. */
export function foldAr(s: string): string {
  return String(s ?? '')
    .replace(MARKS, '')
    .replace(/[أإآٱ]/g, 'ا')
    .replace(/ى/g, 'ي')
    .replace(/ة/g, 'ه')
    .replace(/[\s:：*]+/g, '')
    .toLowerCase()
}

/** Arabic-Indic and Persian digits → ASCII, Arabic decimal separator → ".", thousands separator dropped. */
export function normalizeDigits(s: string): string {
  return String(s ?? '')
    .replace(/[٠-٩]/g, (d) => String(d.charCodeAt(0) - 0x660))
    .replace(/[۰-۹]/g, (d) => String(d.charCodeAt(0) - 0x6f0))
    .replace(/٫/g, '.')
    .replace(/٬/g, '')
}

export type ArabicField = 'stem' | 'type' | 'answer' | 'analysis' | 'subject' | 'chapter' | 'difficulty' | 'score' | 'options'
export type ArabicHeader = { field: ArabicField } | { field: 'option'; index: number }

const RAW_HEADERS: Record<ArabicField, string[]> = {
  stem: ['السؤال', 'سؤال', 'نصالسؤال', 'نصسؤال', 'السوال', 'نصالسوال', 'صيغهالسؤال'],
  type: ['النوع', 'نوع', 'نوعالسؤال', 'نمطالسؤال', 'نمط', 'نوعهالسؤال'],
  answer: ['الاجابه', 'اجابه', 'الاجابهالصحيحه', 'اجابهصحيحه', 'الجواب', 'الجوابالصحيح', 'جواب', 'الاجابهالنموذجيه', 'الجوابالنموذجي'],
  analysis: ['الشرح', 'شرح', 'التفسير', 'التوضيح', 'التعليل', 'التحليل', 'الشرحوالتعليل', 'شرحالاجابه'],
  subject: ['الماده', 'ماده', 'المقرر', 'المساق'],
  chapter: ['الفصل', 'الباب', 'الوحده', 'الوحدهالدراسيه', 'الموضوع', 'المحور', 'الدرس'],
  difficulty: ['الصعوبة', 'صعوبة', 'مستوى الصعوبة', 'درجة الصعوبة'],
  score: ['الدرجه', 'الدرجات', 'درجه', 'درجهالسؤال', 'النقاط', 'نقاط'],
  options: ['الخيارات', 'خيارات', 'الاختيارات', 'اختيارات', 'خياراتالسؤال'],
}

const HEADERS = Object.fromEntries((Object.keys(RAW_HEADERS) as ArabicField[]).map((k) => [k, RAW_HEADERS[k].map(foldAr)])) as Record<ArabicField, string[]>

/** Arabic option letters in order: أ ب ج د هـ و ز ح (folded: ا ب ج د ه و ز ح). */
const LETTERS = ['ا', 'ب', 'ج', 'د', 'ه', 'و', 'ز', 'ح']
const OPTION_HEADER = /^(?:الخيار|خيار|الاختيار|اختيار)(ا|ب|ج|د|ه|و|ز|ح|[a-h]|[1-8])$/

export function classifyArabicHeader(raw: string): ArabicHeader | null {
  const n = foldAr(raw)
  if (!n) return null
  for (const field of Object.keys(HEADERS) as ArabicField[]) {
    if (HEADERS[field].includes(n)) return { field }
  }
  const m = OPTION_HEADER.exec(n)
  if (m) {
    const k = m[1] as string
    const index = /^[1-8]$/.test(k) ? Number(k) - 1 : /^[a-h]$/.test(k) ? k.charCodeAt(0) - 97 : LETTERS.indexOf(k)
    return { field: 'option', index }
  }
  // a bare Arabic letter as the header of an options column
  const bare = LETTERS.indexOf(n)
  if (n.length === 1 && bare >= 0) return { field: 'option', index: bare }
  return null
}

export type ArabicQuestionType = 'single_choice' | 'multi_choice' | 'true_false' | 'fill_blank' | 'short_answer'

/** Question type from an Arabic label, or null when it is not one (multi-answer is tested before single). */
export function arabicTypeFromLabel(raw: string): ArabicQuestionType | null {
  const n = foldAr(raw)
  if (!n) return null
  if (n.includes('اكثر') || (n.includes('متعدد') && n.includes('اجابات')) || n.includes('متعددهالاجابه') || n.includes('اجاباتمتعدده')) return 'multi_choice'
  if (n.includes('اختيارمن') || n.includes('اجابهواحده') || n.includes('اختيارواحد') || n.includes('اختيارمتعدد')) return 'single_choice'
  if ((n.includes('صح') && n.includes('خطا')) || n.includes('صواب')) return 'true_false'
  if (n.includes('فراغ') || n.includes('اكمل') || n.includes('املا') || n.includes('ملء')) return 'fill_blank'
  if (n.includes('قصير') || n.includes('مقال') || n.includes('مفتوح')) return 'short_answer'
  return null
}

/** easy / medium / hard from سهل / متوسط / صعب (and a few synonyms). */
export function arabicDifficulty(raw: string): 'easy' | 'medium' | 'hard' | undefined {
  switch (foldAr(raw)) {
    case 'سهل':
    case 'سهله':
    case 'بسيط': return 'easy'
    case 'متوسط':
    case 'متوسطه':
    case 'وسط': return 'medium'
    case 'صعب':
    case 'صعبه':
    case 'عالي':
    case 'صعبجدا': return 'hard'
    default: return undefined
  }
}

const LETTER_CHAR = /[اأإآبجدهوزح]/
const SEPARATORS = /[\s,،;؛/&+\-–]+/

/**
 * Arabic-letter answers ("أ", "ب، د", "أج", "(ج)", "ب. القاهرة") → Latin letters ("A", "BD", "C", "B"), the form the
 * graders expect. Returns null when the text is not an Arabic-letter answer. A lone "و" between letters is the word
 * "and" ("أ و ج"); on its own, or inside a run like "وز", it is the letter F.
 */
export function arabicLettersToLatin(raw: string): string | null {
  let t = String(raw ?? '').replace(MARKS, '').trim()
  // "ب. القاهرة" / "(ب) القاهرة": one letter, a closing mark, then the option text
  const lead = /^[(\[]?([اأإآبجدهوزح])(?:[)\]]\s*|\s*[.:)\-–]\s*)(\S[\s\S]*)$/.exec(t)
  if (lead && !onlyLetters(lead[2] as string)) return toLatin(lead[1] as string)
  t = t.replace(/[()\[\].:]/g, ' ').trim()
  if (!t) return null
  let tokens = t.split(SEPARATORS).filter(Boolean)
  if (tokens.length > 1) tokens = tokens.filter((x) => x !== 'و')
  if (!tokens.length) return null
  let out = ''
  for (const tok of tokens) {
    for (const ch of tok) {
      if (!LETTER_CHAR.test(ch)) return null
      out += toLatin(ch)
    }
  }
  return [...new Set(out)].sort().join('')
}

function onlyLetters(text: string): boolean {
  const tokens = text.replace(/[()\[\].:]/g, ' ').split(SEPARATORS).filter(Boolean)
  return tokens.length > 0 && tokens.every((tok) => [...tok].every((ch) => LETTER_CHAR.test(ch)))
}

function toLatin(ch: string): string {
  const i = LETTERS.indexOf(foldAr(ch))
  return String.fromCharCode(65 + (i < 0 ? 0 : i))
}

/**
 * The letters a choice answer stands for: Arabic letters, or the exact text of one option ("القاهرة" → the letter of
 * that option). Latin-letter answers are left to the normal normaliser (null here).
 */
export function arabicChoiceAnswer(answer: string, options: string[]): string | null {
  const a = String(answer ?? '').replace(MARKS, '').trim()
  if (!a || /^[A-Ha-h](?:[\s,،;\-+&/]*[A-Ha-h])*\.?$/.test(a)) return null
  const letters = arabicLettersToLatin(a)
  if (letters && letters.charCodeAt(letters.length - 1) - 65 < options.length) return letters
  const folded = foldAr(a)
  const hits = options.map((o, i) => (foldAr(o) === folded ? i : -1)).filter((i) => i >= 0)
  return hits.length === 1 ? String.fromCharCode(65 + (hits[0] as number)) : null
}

/** True/false answers written as صواب (the exam normaliser knows صح / صحيح / خطأ). */
export function arabicTrueFalseAnswer(answer: string): string | null {
  const n = foldAr(answer)
  if (n === 'صواب' || n === 'صائب') return 'صحيح'
  return null
}
