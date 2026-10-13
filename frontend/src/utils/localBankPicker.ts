import type { Question } from '@exameow/shared'
import { normText } from './examBuilder'

// Pure rules of the teacher's "pick a subset of a local bank" panel in the exam editor (the banks live in this browser
// only). Kept out of the component so a plain script can test them.

/** Duplicate key of a question: the Arabic-folded stem with runs of whitespace collapsed (the spreadsheet importer's rule). */
export const stemKey = (stem: string): string => normText(stem).replace(/\s+/g, ' ')

/** The searchable text of a question: stem, options, chapter and answer, folded the same way as the stems. */
function haystack(q: Question): string {
  return normText([q.stem, ...(q.options ?? []), q.chapter ?? '', q.answer ?? ''].join('\n'))
}

/**
 * Indexes (into `questions`) of the questions that match the search text. Every word of the query must occur
 * somewhere in the question (Arabic letter variants and case do not matter). An empty query matches everything.
 */
export function filterLocal(questions: Question[], query: string): number[] {
  const words = normText(query).split(/\s+/).filter(Boolean)
  const out: number[] = []
  questions.forEach((q, i) => {
    if (!words.length) {
      out.push(i)
      return
    }
    const text = haystack(q)
    if (words.every((w) => text.includes(w))) out.push(i)
  })
  return out
}

/** For each bank question: it is already in the exam (same stem), so it cannot be ticked again. */
export function takenMask(questions: Question[], existing: Question[]): boolean[] {
  const inExam = new Set(existing.map((q) => stemKey(q.stem)))
  return questions.map((q) => inExam.has(stemKey(q.stem)))
}

export interface CapState {
  /** questions the exam holds after adding the ticked ones */
  total: number
  /** how many more can still be ticked */
  room: number
  /** the exam would exceed the cap */
  over: number
  /** at or above 90 % of the cap (and not over): a warning */
  near: boolean
  full: boolean
}

/** Cap arithmetic: `inExam` questions are already in the exam, `selected` are ticked, `cap` is the exam's limit. */
export function capState(inExam: number, selected: number, cap: number): CapState {
  const total = inExam + selected
  return {
    total,
    room: Math.max(0, cap - total),
    over: Math.max(0, total - cap),
    near: total <= cap && total >= Math.ceil(cap * 0.9),
    full: total >= cap,
  }
}

/**
 * "Select all" for the filtered list: ticks the questions that can still be ticked (not taken, not already ticked) in order
 * until the exam's room is used up. `skipped` counts those that did not fit, so the panel can say so.
 */
export function selectAllWithin(
  current: ReadonlySet<number>,
  visible: number[],
  taken: boolean[],
  room: number,
): { next: Set<number>; skipped: number } {
  const next = new Set(current)
  let left = Math.max(0, room)
  let skipped = 0
  for (const i of visible) {
    if (taken[i] || next.has(i)) continue
    if (left <= 0) {
      skipped++
      continue
    }
    next.add(i)
    left--
  }
  return { next, skipped }
}

/** "Select none" for the filtered list: unticks the visible questions and keeps ticks made under other filters. */
export function selectNoneOf(current: ReadonlySet<number>, visible: number[]): Set<number> {
  const hide = new Set(visible)
  return new Set([...current].filter((i) => !hide.has(i)))
}

/** The ticked questions of the bank, in bank order, as plain copies (the editor gives them fresh ids). */
export function chosenQuestions(questions: Question[], selected: ReadonlySet<number>): Question[] {
  return [...selected]
    .filter((i) => i >= 0 && i < questions.length)
    .sort((a, b) => a - b)
    .map((i) => ({ ...(questions[i] as Question) }))
}
