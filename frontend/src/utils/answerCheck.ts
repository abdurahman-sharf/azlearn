// The practice page checks answers on the student's device, with the SAME rules the server grades a real attempt with
// (packages/server/src/platform_exams.rs: `auto_grade`, `norm_text`, `norm_fill`, `canon_tf`; relay.rs: `grade`,
// `normalize_choice`, `is_true_answer`). Keep the two in step: the fill-blank half is pinned by the shared vectors in
// packages/shared/fixtures/fill-blank-vectors.json, which a Rust test and this file's test both read.
// Short answers are never checked automatically (a teacher grades them): the practice page lets the student compare with
// the model answer and mark themselves.

export type CheckResult = 'correct' | 'incorrect' | 'unanswered' | 'manual'

/** Unicode White_Space, as Rust's `char::is_whitespace` / `str::trim` see it (JavaScript's `\s` differs at U+0085 and U+FEFF). */
function isWhitespace(cp: number): boolean {
  return (cp >= 0x09 && cp <= 0x0d) || cp === 0x20 || cp === 0x85 || cp === 0xa0 || cp === 0x1680 || (cp >= 0x2000 && cp <= 0x200a) ||
    cp === 0x2028 || cp === 0x2029 || cp === 0x202f || cp === 0x205f || cp === 0x3000
}

function trimRust(s: string): string {
  const chars = [...s]
  let a = 0
  let b = chars.length
  while (a < b && isWhitespace(chars[a]!.codePointAt(0)!)) a++
  while (b > a && isWhitespace(chars[b - 1]!.codePointAt(0)!)) b--
  return chars.slice(a, b).join('')
}

/** Folding of one character: null = dropped (a diacritic or a tatweel), alef and alef-maqsura forms are unified. */
function foldChar(ch: string): string | null {
  const cp = ch.codePointAt(0)!
  if ((cp >= 0x064b && cp <= 0x065f) || cp === 0x0670 || cp === 0x0640) return null
  if (ch === 'أ' || ch === 'إ' || ch === 'آ' || ch === 'ٱ') return 'ا'
  if (ch === 'ى') return 'ي'
  return ch
}

/** Arabic-aware comparison form: trims, strips diacritics/tatweel, unifies alef/yeh variants, lowercases. */
export function normText(s: string): string {
  let out = ''
  for (const ch of trimRust(s)) {
    const f = foldChar(ch)
    if (f !== null) out += f
  }
  return out.toLowerCase()
}

/** Zero-width and bidirectional control marks that copy-paste leaves in a typed answer. */
function isInvisibleMark(cp: number): boolean {
  return (cp >= 0x200b && cp <= 0x200f) || (cp >= 0x202a && cp <= 0x202e) || (cp >= 0x2060 && cp <= 0x2064) || (cp >= 0x2066 && cp <= 0x2069) || cp === 0xfeff
}

/** Punctuation that is stripped from the START and the END of a fill-blank answer (inner punctuation stays). */
const EDGE_PUNCTUATION = new Set(['.', ',', ';', ':', '!', '?', '،', '؛', '؟', '"', "'", '“', '”', '‘', '’', '«', '»', '(', ')', '[', ']', '{', '}', '-', '–', '—', '_'])

const isDash = (ch: string): boolean => ch === '-' || ch === '–' || ch === '—'
const isDigit = (ch: string | undefined): boolean => ch !== undefined && ch >= '0' && ch <= '9'
/** Rust's `char::is_alphanumeric`: Alphabetic or Numeric (Nd, Nl, No). */
const isAlphanumeric = (ch: string | undefined): boolean => ch !== undefined && /[\p{Alphabetic}\p{N}]/u.test(ch)

function foldDigit(ch: string): string {
  const cp = ch.codePointAt(0)!
  if (cp >= 0x0660 && cp <= 0x0669) return String(cp - 0x0660)
  if (cp >= 0x06f0 && cp <= 0x06f9) return String(cp - 0x06f0)
  if (cp === 0x066b) return '.'
  if (cp === 0x066c) return ','
  if (cp === 0x2212) return '-' // the true minus sign
  return ch
}

/**
 * Strips whitespace and edge punctuation from both ends, repeatedly and in any mix, except: at the START a hyphen / dash
 * directly followed by a digit (or by `.` and a digit) is the sign of a number and a `.` directly followed by a digit is its
 * decimal point (`-5` and `.5` are not `5`); at the END a hyphen / dash directly after a letter or a digit belongs to it
 * (`Cl-`: the ion is not the atom). A dash that stands apart (`مصر -`) is decoration. Mirrors `trim_fill_edges` (Rust).
 */
function trimFillEdges(s: string): string {
  const chars = [...s]
  const strip = (ch: string): boolean => ch === ' ' || isWhitespace(ch.codePointAt(0)!) || EDGE_PUNCTUATION.has(ch)
  let a = 0
  while (a < chars.length && strip(chars[a]!)) {
    const c = chars[a]!
    const numericSign = isDash(c) && (isDigit(chars[a + 1]) || (chars[a + 1] === '.' && isDigit(chars[a + 2])))
    const decimalPoint = c === '.' && isDigit(chars[a + 1])
    if (numericSign || decimalPoint) break
    a++
  }
  let b = chars.length
  while (b > a && strip(chars[b - 1]!)) {
    if (isDash(chars[b - 1]!) && b - 1 > a && isAlphanumeric(chars[b - 2])) break
    b--
  }
  return chars.slice(a, b).join('')
}

/**
 * The comparison form of a fill-blank answer: [`normText`], then Arabic-Indic and Persian digits become 0-9 (the Arabic
 * decimal separator a dot and the thousands separator a comma, the true minus sign U+2212 a hyphen), invisible marks go,
 * every run of whitespace (a no-break space too) becomes one space, and punctuation at either end is stripped (see
 * [`trimFillEdges`]: the sign of a number and a hyphen glued to a word stay). ة and ه are deliberately NOT folded.
 */
export function normFill(s: string): string {
  let t = ''
  for (const ch of normText(s)) {
    if (isInvisibleMark(ch.codePointAt(0)!)) continue
    t += foldDigit(ch)
  }
  let spaced = ''
  let inSpace = false
  for (const ch of t) {
    if (isWhitespace(ch.codePointAt(0)!)) {
      if (!inSpace) spaced += ' '
      inSpace = true
    } else {
      spaced += ch
      inSpace = false
    }
  }
  return trimFillEdges(spaced)
}

/**
 * A fill-blank answer is right when it equals the model answer or one of its `a|b` alternatives, after [`normFill`]. An answer
 * that is nothing but punctuation can only match a key (alternative) that is nothing but punctuation too, compared as typed
 * ([`normText`]), the way the server does it.
 */
export function fillMatches(answer: string, given: string): boolean {
  const u = normFill(given)
  if (u === '') {
    const plain = normText(given)
    return plain !== '' && answer.split('|').some((alt) => normFill(alt) === '' && normText(alt) === plain)
  }
  return answer.split('|').some((alt) => normFill(alt) === u)
}

/** Maps Arabic true/false words onto the A/B convention. */
export function canonTf(s: string): string {
  switch (normText(s)) {
    case 'صح':
    case 'صحيح':
    case 'نعم':
      return 'A'
    case 'خطا':
    case 'خاطئ':
    case 'لا':
    case 'غير صحيح':
    case 'خطاء':
      return 'B'
    default:
      return s
  }
}

const TRUE_ALIASES = ['A', '√', '对', '正确', 'TRUE', 'T', '是', 'YES', 'Y', '1']
const NEGATIONS = ['不', '非', '错', '没']

/** The server's "does this text say true" test (a longer alias may be contained in the text unless a negation is left over). */
export function isTrueAnswer(a: string): boolean {
  const t = trimRust(a)
  const upper = t.toUpperCase()
  return TRUE_ALIASES.some(
    (v) => upper === v.toUpperCase() || ([...v].length > 1 && t.includes(v) && ![...t.replace(v, '')].some((c) => NEGATIONS.includes(c))),
  )
}

/** The letters A-J of a choice answer, sorted ("c, a" and "AC" are the same answer). */
export function normalizeChoice(s: string): string {
  return [...trimRust(s).toUpperCase()].filter((c) => c >= 'A' && c <= 'J').sort().join('')
}

export interface Checkable {
  type: string
  answer: string
}

/** Whether a piece of text is empty for grading (the server trims first). */
export const isBlank = (s: string | null | undefined): boolean => trimRust(s ?? '') === ''

/**
 * Checks one answer the way the server grades it. `unanswered` for a blank answer, `manual` for a short answer (a teacher
 * grades those), otherwise `correct` / `incorrect`.
 */
export function checkAnswer(q: Checkable, given: string | null | undefined): CheckResult {
  if (q.type === 'short_answer') return 'manual'
  const g = given ?? ''
  if (isBlank(g)) return 'unanswered'
  switch (q.type) {
    case 'single_choice':
    case 'multi_choice':
      return normalizeChoice(g) === normalizeChoice(q.answer) ? 'correct' : 'incorrect'
    case 'true_false': {
      const u = canonTf(g)
      if (isBlank(u)) return 'unanswered'
      return isTrueAnswer(u) === isTrueAnswer(canonTf(q.answer)) ? 'correct' : 'incorrect'
    }
    case 'fill_blank':
      return fillMatches(q.answer, g) ? 'correct' : 'incorrect'
    default:
      return 'manual'
  }
}

// ── how an answer is written out for the student ─────────────────────────────────────────────────────────────────────────

export interface AnswerLabels {
  /** the default texts of a true/false question without options of its own */
  trueText: string
  falseText: string
}

export interface ChoiceItem {
  letter: string
  text: string
}

export type AnswerView =
  /** a choice or true/false answer: the letters with the text of each chosen option */
  | { kind: 'choice'; items: ChoiceItem[] }
  | { kind: 'text'; text: string }
  | { kind: 'none' }

/**
 * The option text for each letter of a choice answer, in option order. A letter without an option (the answer names C but
 * only two options exist) keeps an empty text instead of being dropped, so nothing the student chose disappears.
 */
export function choiceItems(letters: string, options: string[]): ChoiceItem[] {
  return [...normalizeChoice(letters)].map((letter) => ({ letter, text: options[letter.charCodeAt(0) - 65] ?? '' }))
}

/**
 * Writes `value` (a student's answer, or a model answer) for display: letters plus option text for choice questions
 * (true/false shows صحيح/خطأ unless the question has options of its own), the text itself for fill-blank and short answers.
 * `a|b` alternatives of a model fill-blank answer are listed with " / ".
 */
export function describeAnswer(q: { type: string; options: string[] }, value: string | null | undefined, labels: AnswerLabels, model = false): AnswerView {
  if (isBlank(value)) return { kind: 'none' }
  const v = value as string
  if (q.type === 'single_choice' || q.type === 'multi_choice') return { kind: 'choice', items: choiceItems(v, q.options) }
  if (q.type === 'true_false') {
    const letter = isTrueAnswer(canonTf(v)) ? 'A' : 'B'
    const own = q.options.filter((o) => o.trim() !== '')
    const text = own.length >= 2 ? own[letter === 'A' ? 0 : 1]! : letter === 'A' ? labels.trueText : labels.falseText
    return { kind: 'choice', items: [{ letter, text }] }
  }
  return { kind: 'text', text: model && q.type === 'fill_blank' ? v.split('|').map((x) => x.trim()).filter(Boolean).join(' / ') : v }
}
