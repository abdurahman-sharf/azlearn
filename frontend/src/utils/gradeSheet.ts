// Pure helpers behind the grading screens (GradeExamView, AttemptView): what the grader typed per answer, how it differs
// from what the server holds, validation and batching. No Vue, no network - tested as a plain script.
//
// Two maps with the same shape are kept per screen: the boxes the grader sees (`CellMap`) and the server's copy of the
// same answers (`ServerMap`). Both are keyed question id -> attempt id, so switching to another question never loses
// what was typed, and a save sends exactly the answers whose boxes differ from the server.

/** The server rejects a longer comment (`feedback_too_long`). Counted in characters, like the server does. */
export const FEEDBACK_MAX = 500
/** One `grade-batch` call takes at most this many items (one transaction). */
export const BATCH_MAX = 500

/** What the grader sees in the boxes of one answer. Strings: an empty points box means "nothing typed". */
export interface GradeCell { points: string; feedback: string }
/** What the server holds for that answer (`points` is null while the answer waits for a grade). */
export interface ServerCell { points: number | null; feedback: string | null; max: number }
export type CellMap = Map<string, Map<string, GradeCell>>
export type ServerMap = Map<string, Map<string, ServerCell>>

/** One changed answer, in the shape of a `grade-batch` item. A field that is absent is left as it is on the server. */
export interface GradeItem { attempt_id: string; question_id: string; points?: number; feedback?: string }

export interface SheetAnswer { attempt_id: string; points: number | null; feedback?: string | null }
export interface SheetQuestion { id: string; max: number; pending: number; answers: SheetAnswer[] }

export const round2 = (n: number): number => Math.round(n * 100) / 100
/** Characters (code points), the way the server counts them - not UTF-16 units. */
export const charCount = (s: string): number => Array.from(s).length

// What the server never stores in a comment (`platform_exams::clean_feedback` - keep the two in step): control characters
// other than a line break or a tab (CR is dropped, so CR/LF reads as LF), the zero-width space and the left/right marks,
// the bidi embedding / override / isolate controls, the word joiner and invisible operators, and the byte order mark.
// The zero-width joiner and non-joiner stay (scripts and emoji need them) unless they are all that is left.
// eslint-disable-next-line no-control-regex
const HIDDEN_CHARS = /[\u0000-\u0008\u000B-\u001F\u007F-\u009F\u200B\u200E\u200F\u202A-\u202E\u2060-\u2064\u2066-\u2069\uFEFF]/g
/**
 * The comment as the server will keep it: hidden characters dropped, then trimmed; `''` when nothing visible is left
 * (that is how a comment is cleared). The counter, the diff against the server and the 500 limit all use this text, so
 * the box can never say "ok" for something the server refuses, or "changed" for something it would store unchanged.
 */
export function cleanFeedback(raw: string): string {
  const text = raw.replace(HIDDEN_CHARS, '').trim()
  return /^[\s\u200C\u200D]*$/.test(text) ? '' : text
}

/** The boxes as they start for a server value. */
export function cellOf(s: ServerCell): GradeCell {
  return { points: s.points === null ? '' : String(s.points), feedback: s.feedback ?? '' }
}

export type PointsProblem = 'format' | 'range' | null
/** A points box is fine when empty (untouched) or a number from 0 to the question's full marks. */
export function pointsProblem(text: string, max: number): PointsProblem {
  const t = text.trim()
  if (t === '') return null
  const n = Number(t)
  if (!Number.isFinite(n)) return 'format'
  return n < 0 || n > max ? 'range' : null
}
export const feedbackTooLong = (text: string): boolean => charCount(cleanFeedback(text)) > FEEDBACK_MAX

/**
 * What differs between the boxes and the server: `points` when a number was typed that is not the stored one (an empty
 * box is untouched - a grade cannot be taken back), `feedback` when the cleaned text ([`cleanFeedback`]) differs (an
 * empty box over a stored comment means "remove it" and is sent as `""`). A points box that is not a number stays in the result as NaN so that
 * validation reports it instead of silently dropping it.
 */
export function diffCell(cell: GradeCell, server: ServerCell): { points?: number; feedback?: string } {
  const out: { points?: number; feedback?: string } = {}
  const t = cell.points.trim()
  if (t !== '') {
    const n = Number(t)
    if (server.points === null || round2(n) !== server.points) out.points = n
  }
  const f = cleanFeedback(cell.feedback)
  if (f !== cleanFeedback(server.feedback ?? '')) out.feedback = f
  return out
}

const changed = (d: { points?: number; feedback?: string }): boolean => d.points !== undefined || d.feedback !== undefined

/** Every answer of the screen whose boxes differ from the server, in sheet order. */
export function buildItems(state: CellMap, base: ServerMap): GradeItem[] {
  const items: GradeItem[] = []
  for (const [qid, answers] of base) {
    const cells = state.get(qid)
    if (!cells) continue
    for (const [aid, server] of answers) {
      const cell = cells.get(aid)
      if (!cell) continue
      const d = diffCell(cell, server)
      if (changed(d)) items.push({ attempt_id: aid, question_id: qid, ...d })
    }
  }
  return items
}

export interface GradeProblem { code: 'points' | 'feedback'; question_id: string; attempt_id: string }
/** The first item the server would refuse (points outside 0..max or not a number, a comment over the limit), or null. */
export function validateItems(items: GradeItem[], base: ServerMap): GradeProblem | null {
  for (const it of items) {
    const max = base.get(it.question_id)?.get(it.attempt_id)?.max ?? 0
    const at = { question_id: it.question_id, attempt_id: it.attempt_id }
    if (it.points !== undefined && (!Number.isFinite(it.points) || it.points < 0 || it.points > max)) return { code: 'points', ...at }
    if (it.feedback !== undefined && charCount(it.feedback) > FEEDBACK_MAX) return { code: 'feedback', ...at }
  }
  return null
}

/** Splits the items into calls of at most `size` (the server's per-call limit). */
export function chunkItems<T>(items: T[], size: number = BATCH_MAX): T[][] {
  const out: T[][] = []
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size))
  return out
}

/** How many changed answers each question has (the question buttons mark the ones with unsaved work). */
export function countByQuestion(items: GradeItem[]): Map<string, number> {
  const m = new Map<string, number>()
  for (const it of items) m.set(it.question_id, (m.get(it.question_id) ?? 0) + 1)
  return m
}

/**
 * The index of the next question (after `from`, wrapping round, never `from` itself) that still has answers waiting
 * for a grade, or -1 when there is none.
 */
export function nextWork(questions: { pending: number }[], from: number): number {
  const n = questions.length
  for (let step = 1; step < n; step++) {
    const i = (((from + step) % n) + n) % n
    if ((questions[i]?.pending ?? 0) > 0) return i
  }
  return -1
}

/**
 * Takes a freshly loaded sheet in. An answer the grader has not touched follows the server (so it shows what a
 * colleague or an earlier save wrote); one the grader changed keeps what was typed and is compared with the new server value.
 */
export function mergeSheet(state: CellMap, base: ServerMap, questions: SheetQuestion[]): void {
  for (const q of questions) {
    // read the inner maps back after creating them: on a reactive map `get` returns the tracked proxy, while the map
    // that was just created is the raw one (writing through it would not notify the screen)
    if (!state.has(q.id)) state.set(q.id, new Map())
    if (!base.has(q.id)) base.set(q.id, new Map())
    const cells = state.get(q.id)!
    const servers = base.get(q.id)!
    for (const a of q.answers) {
      const next: ServerCell = { points: a.points, feedback: a.feedback ?? null, max: q.max }
      const old = servers.get(a.attempt_id)
      const cell = cells.get(a.attempt_id)
      if (!cell || !old || !changed(diffCell(cell, old))) cells.set(a.attempt_id, cellOf(next))
      servers.set(a.attempt_id, next)
    }
  }
}

/** After the server accepted `items`: its copy of those answers now equals what was sent, so they stop counting as changes. */
export function markSaved(base: ServerMap, items: GradeItem[]): void {
  for (const it of items) {
    const s = base.get(it.question_id)?.get(it.attempt_id)
    if (!s) continue
    if (it.points !== undefined) s.points = round2(it.points)
    if (it.feedback !== undefined) s.feedback = it.feedback === '' ? null : it.feedback
  }
}

/**
 * The bulk tools: puts full marks (or zero) into the points box of every listed answer that still waits for a grade
 * AND whose box is still empty. A grade that exists already is left alone - correcting one is a deliberate act, never a
 * side effect of a bulk click - and so is a grade the grader has typed and not saved yet: "fill in the rest" must not
 * overwrite what was just decided, with no way back. Returns how many boxes were set.
 */
export function fillWaiting(state: CellMap, base: ServerMap, qid: string, attemptIds: string[], mode: 'full' | 'zero'): number {
  let n = 0
  for (const aid of attemptIds) {
    const server = base.get(qid)?.get(aid)
    const cell = state.get(qid)?.get(aid)
    if (!server || !cell || server.points !== null || cell.points.trim() !== '') continue
    cell.points = mode === 'full' ? String(server.max) : '0'
    n++
  }
  return n
}

/**
 * Answers of the previous sheet that the fresh one no longer lists - graded by a colleague meanwhile, or hidden by
 * "ungraded only" - but whose boxes hold unsaved edits. They are put back into `next` so they stay on screen: the
 * save set is exactly what the grader can see, never a correction typed earlier and forgotten behind a filter. An edit
 * leaves this list when it is saved (the server copy then equals the box) or typed back to the server's value.
 * Mutates `next`; returns how many answers were kept.
 */
export function keepEdited<A extends { attempt_id: string }>(
  prev: { id: string; answers: A[] }[] | undefined,
  next: { id: string; answers: A[] }[],
  state: CellMap,
  base: ServerMap,
): number {
  if (!prev) return 0
  let kept = 0
  for (const pq of prev) {
    const nq = next.find((q) => q.id === pq.id)
    if (!nq) continue
    const listed = new Set(nq.answers.map((a) => a.attempt_id))
    for (const a of pq.answers) {
      if (listed.has(a.attempt_id)) continue
      const cell = state.get(pq.id)?.get(a.attempt_id)
      const server = base.get(pq.id)?.get(a.attempt_id)
      if (cell && server && changed(diffCell(cell, server))) {
        nq.answers.push(a)
        kept++
      }
    }
  }
  return kept
}

/** The body of the single-attempt call (`PATCH /attempts/{id}/grade`): `grades` and `feedback` keyed by question id. */
export function toAttemptBody(items: GradeItem[]): { grades?: Record<string, number>; feedback?: Record<string, string> } {
  const grades: Record<string, number> = {}
  const feedback: Record<string, string> = {}
  for (const it of items) {
    if (it.points !== undefined) grades[it.question_id] = it.points
    if (it.feedback !== undefined) feedback[it.question_id] = it.feedback
  }
  return { ...(Object.keys(grades).length ? { grades } : {}), ...(Object.keys(feedback).length ? { feedback } : {}) }
}

/** One result item of an attempt as a sheet question with a single answer (the attempt page reuses the same helpers). */
export function sheetFromAttempt(
  attemptId: string,
  items: { id: string; max: number; correct: boolean | null; points: number; feedback?: string | null }[],
): SheetQuestion[] {
  return items.map((it) => ({
    id: it.id,
    max: it.max,
    pending: it.correct === null ? 1 : 0,
    answers: [{ attempt_id: attemptId, points: it.correct === null ? null : it.points, feedback: it.feedback ?? null }],
  }))
}
