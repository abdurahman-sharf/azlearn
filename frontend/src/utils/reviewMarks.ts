// "Marked for review" and the questions still without an answer: the take page's question navigator and the list the
// submit dialog shows before the student hands the attempt in. Indexes are the zero-based positions in the displayed order.

type Q = { id: string }

export function unansweredIndexes(questions: readonly Q[], answers: Readonly<Record<string, string>>): number[] {
  const out: number[] = []
  questions.forEach((q, i) => {
    if ((answers[q.id] ?? '').trim() === '') out.push(i)
  })
  return out
}

export function markedIndexes(questions: readonly Q[], marked: ReadonlySet<string>): number[] {
  const out: number[] = []
  questions.forEach((q, i) => {
    if (marked.has(q.id)) out.push(i)
  })
  return out
}

/** A new set with `id` marked, or unmarked when it was marked (the page never mutates its state in place). */
export function toggleMarked(marked: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(marked)
  if (!next.delete(id)) next.add(id)
  return next
}

/** Marks that still belong to a question of the page (a stored list may name questions that are gone). */
export function pruneMarked(marked: ReadonlySet<string>, questions: readonly Q[]): Set<string> {
  const ids = new Set(questions.map((q) => q.id))
  return new Set([...marked].filter((id) => ids.has(id)))
}

export const serializeMarked = (marked: ReadonlySet<string>): string => JSON.stringify([...marked])

/** Reads a stored list defensively: anything that is not a list of text is an empty set. */
export function parseMarked(raw: string | null | undefined): Set<string> {
  if (!raw) return new Set()
  try {
    const v: unknown = JSON.parse(raw)
    return Array.isArray(v) ? new Set(v.filter((x): x is string => typeof x === 'string')) : new Set()
  } catch {
    return new Set()
  }
}

export type NavState = 'answered' | 'unanswered'

/** What the navigator button of question `id` says (in words as well as in its look). */
export function navState(id: string, answers: Readonly<Record<string, string>>): NavState {
  return (answers[id] ?? '').trim() !== '' ? 'answered' : 'unanswered'
}
