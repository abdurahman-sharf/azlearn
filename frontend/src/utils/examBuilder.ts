import type { Question } from '@exameow/shared'

/** A question inside the exam being built; `src` is the bank item it was copied from (statistics only). */
export type ExamQuestion = Question & { src?: string }

export const letter = (i: number): string => String.fromCharCode(65 + i)

/** Arabic-aware comparison form, mirroring the server's `norm_text`. */
export function normText(s: string): string {
  return s.trim().replace(/[ً-ٰٟـ]/g, '').replace(/[أإآٱ]/g, 'ا').replace(/ى/g, 'ي').toLowerCase()
}

const TRUE_WORDS = ['a', 'true', 't', 'yes', 'y', '1', '√', '对', '正确', '是', 'صح', 'صحيح', 'نعم']
const FALSE_WORDS = ['b', 'false', 'f', 'no', 'n', '0', '×', '错', '错误', '否', 'خطا', 'خاطي', 'خاطئ', 'خطاء', 'لا', 'غير صحيح']

/**
 * Brings an answer into the form the graders expect (same rules as the server): choice questions →
 * sorted letters ("AC"), true/false → "A"/"B". Returns null when it cannot be made valid.
 */
export function normalizeAnswer(type: string, options: string[], answer: string): string | null {
  if (type === 'single_choice' || type === 'multi_choice') {
    // leading letters/separators only, so "B. القاهرة" and "A, C" both work
    const m = answer.trim().match(/^[A-Za-z,،;؛、/&+\- ]*/)
    const letters = [...new Set([...(m?.[0] ?? '')].filter((c) => /[A-Za-z]/.test(c)).map((c) => c.toUpperCase()))].sort()
    if (!letters.length || !letters.every((c) => c.charCodeAt(0) - 65 < options.length)) return null
    if (type === 'single_choice' && letters.length !== 1) return null
    return letters.join('')
  }
  if (type === 'true_false') {
    const n = normText(answer)
    return TRUE_WORDS.includes(n) ? 'A' : FALSE_WORDS.includes(n) ? 'B' : null
  }
  return answer.trim()
}

export type Problem = 'stem' | 'options' | 'answer' | 'score'

/** Instant checks shown in the review step (the server repeats them authoritatively). */
export function validateQuestion(q: Question): Problem | null {
  if (!q.stem.trim() || q.stem.length > 3000) return 'stem'
  const isChoice = q.type === 'single_choice' || q.type === 'multi_choice'
  const opts = q.options.map((o) => o.trim()).filter(Boolean)
  if (isChoice && (opts.length < 2 || opts.length > 10)) return 'options'
  if (!q.answer.trim() || normalizeAnswer(q.type, q.type === 'true_false' ? opts : opts, q.answer) === null) return 'answer'
  if (q.score !== undefined && (!Number.isFinite(q.score) || q.score < 0 || q.score > 100)) return 'score'
  return null
}

/** Turns raw (e.g. AI-generated) questions into exam questions: fresh ids, normalised answers, default score. */
export function adoptQuestions(raw: Question[], nextId: () => string, src?: (q: Question) => string | undefined): ExamQuestion[] {
  return raw.map((q) => {
    const options = (q.options ?? []).map((o) => String(o))
    const answer = normalizeAnswer(q.type, options, String(q.answer ?? '')) ?? String(q.answer ?? '')
    const out: ExamQuestion = {
      id: nextId(),
      type: q.type,
      stem: String(q.stem ?? ''),
      options: q.type === 'true_false' && !options.length ? ['صحيح', 'خطأ'] : options,
      answer,
      analysis: String(q.analysis ?? ''),
      score: q.score ?? 1,
    }
    if (q.chapter) out.chapter = String(q.chapter)
    if (q.difficulty) out.difficulty = q.difficulty
    const s = src?.(q)
    if (s) out.src = s
    return out
  })
}

export function totals(qs: Question[]): { points: number; byType: Record<string, number> } {
  const byType: Record<string, number> = {}
  let points = 0
  for (const q of qs) {
    byType[q.type] = (byType[q.type] ?? 0) + 1
    points += q.score ?? 1
  }
  return { points: Math.round(points * 100) / 100, byType }
}

export function move<T>(arr: T[], from: number, delta: number): T[] {
  const to = from + delta
  if (to < 0 || to >= arr.length) return arr
  const copy = arr.slice()
  const [x] = copy.splice(from, 1)
  copy.splice(to, 0, x as T)
  return copy
}

/** `datetime-local` value (local time) for an epoch-ms timestamp, or '' when unset. */
export function toLocalInput(ms: number | null | undefined): string {
  if (!ms) return ''
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
}

export function fromLocalInput(v: string): number | null {
  if (!v) return null
  const t = new Date(v).getTime()
  return Number.isNaN(t) ? null : t
}
