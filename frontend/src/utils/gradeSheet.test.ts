import { computed, reactive } from 'vue'
import {
  BATCH_MAX, FEEDBACK_MAX, buildItems, cellOf, charCount, chunkItems, cleanFeedback, countByQuestion, diffCell, feedbackTooLong, fillWaiting,
  keepEdited, markSaved, mergeSheet, nextWork, pointsProblem, sheetFromAttempt, toAttemptBody, validateItems,
  type CellMap, type ServerMap, type SheetQuestion,
} from './gradeSheet.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const sheet = (): SheetQuestion[] => [
  { id: 'q1', max: 5, pending: 2, answers: [{ attempt_id: 'a1', points: null }, { attempt_id: 'a2', points: null }] },
  { id: 'q2', max: 2, pending: 0, answers: [{ attempt_id: 'a1', points: 2, feedback: 'جيد' }] },
  { id: 'q3', max: 4, pending: 1, answers: [{ attempt_id: 'a3', points: null }] },
]
function fresh(): { state: CellMap; base: ServerMap } {
  const state: CellMap = new Map()
  const base: ServerMap = new Map()
  mergeSheet(state, base, sheet())
  return { state, base }
}
const type = (s: CellMap, q: string, a: string, p: Partial<{ points: string; feedback: string }>): void => {
  const c = s.get(q)?.get(a)
  if (!c) throw new Error('no cell')
  Object.assign(c, p)
}

// --- boxes start as the server's values ---------------------------------------------------------------------------------
eq(cellOf({ points: null, feedback: null, max: 5 }), { points: '', feedback: '' }, 'waiting answer: empty boxes')
eq(cellOf({ points: 2.5, feedback: 'ok', max: 5 }), { points: '2.5', feedback: 'ok' }, 'graded answer: its values')
{
  const { state, base } = fresh()
  eq(buildItems(state, base), [], 'nothing typed: nothing to save')
  eq(state.get('q2')?.get('a1'), { points: '2', feedback: 'جيد' }, 'a graded answer shows its grade and comment')
}

// --- empty points box = untouched; typed values become items ------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q1', 'a1', { points: '4' })
  type(state, 'q1', 'a2', { points: '   ' })
  eq(buildItems(state, base), [{ attempt_id: 'a1', question_id: 'q1', points: 4 }], 'only the typed box is sent')
  type(state, 'q1', 'a1', { points: '' })
  eq(buildItems(state, base), [], 'emptying the box again leaves the answer untouched')
}

// --- only changes are sent -----------------------------------------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q2', 'a1', { points: '2.0' })
  eq(buildItems(state, base), [], 'the same number written differently is no change')
  type(state, 'q2', 'a1', { points: '1.5' })
  eq(buildItems(state, base), [{ attempt_id: 'a1', question_id: 'q2', points: 1.5 }], 'a correction of a graded answer')
  type(state, 'q2', 'a1', { points: '2.004' })
  eq(buildItems(state, base), [], 'points are compared after the server rounding (two decimals)')
}

// --- feedback: changed, cleared, trimmed -----------------------------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q1', 'a2', { feedback: '  أحسنت  ' })
  eq(buildItems(state, base), [{ attempt_id: 'a2', question_id: 'q1', feedback: 'أحسنت' }], 'a comment on a waiting answer, trimmed; the answer stays waiting')
  type(state, 'q2', 'a1', { feedback: '' })
  eq(buildItems(state, base).find((i) => i.question_id === 'q2'), { attempt_id: 'a1', question_id: 'q2', feedback: '' }, 'emptying a stored comment sends "" (clears it)')
  type(state, 'q2', 'a1', { feedback: ' جيد ' })
  eq(buildItems(state, base).some((i) => i.question_id === 'q2'), false, 'the same comment with spaces is no change')
  type(state, 'q3', 'a3', { points: '3', feedback: 'x' })
  eq(buildItems(state, base).find((i) => i.question_id === 'q3'), { attempt_id: 'a3', question_id: 'q3', points: 3, feedback: 'x' }, 'both fields in one item')
}

// --- switching questions keeps what was typed; a reload follows the server for untouched answers -------------------------------
{
  const { state, base } = fresh()
  type(state, 'q1', 'a1', { points: '5' })
  const again = sheet()
  again[0]!.answers[1] = { attempt_id: 'a2', points: 3 } // a colleague graded a2 meanwhile
  mergeSheet(state, base, again)
  eq(state.get('q1')?.get('a1')?.points, '5', 'a typed box survives a reload')
  eq(state.get('q1')?.get('a2')?.points, '3', 'an untouched box follows the server')
  eq(buildItems(state, base), [{ attempt_id: 'a1', question_id: 'q1', points: 5 }], 'the colleague\'s grade is not re-sent')
  // the grader typed over a2, then the server value changed again: the typed value stays and is compared with the new one
  type(state, 'q1', 'a2', { points: '4' })
  again[0]!.answers[1] = { attempt_id: 'a2', points: 4 }
  mergeSheet(state, base, again)
  eq(buildItems(state, base).some((i) => i.attempt_id === 'a2'), false, 'typed value equal to the new server value is no change')
}

// --- filtered reload: answers that left the screen are kept -----------------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q2', 'a1', { points: '1' })
  const onlyPending = sheet().map((q) => ({ ...q, answers: q.answers.filter((a) => a.points === null) }))
  mergeSheet(state, base, onlyPending)
  eq(buildItems(state, base), [{ attempt_id: 'a1', question_id: 'q2', points: 1 }], 'a typed correction survives the "ungraded only" filter')
}

// --- after a successful save the server copy equals what was sent -------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q1', 'a1', { points: '4', feedback: 'جيد' })
  type(state, 'q2', 'a1', { feedback: '' })
  const items = buildItems(state, base)
  eq(items.length, 2, 'two changed answers')
  markSaved(base, items)
  eq(buildItems(state, base), [], 'saved answers are no longer changes (even when they left the screen)')
  eq(base.get('q2')?.get('a1')?.feedback, null, 'a cleared comment is null')
  // a partial save: only the first chunk was accepted
  type(state, 'q1', 'a2', { points: '1' })
  type(state, 'q3', 'a3', { points: '2' })
  const both = buildItems(state, base)
  markSaved(base, both.slice(0, 1))
  eq(buildItems(state, base), [{ attempt_id: 'a3', question_id: 'q3', points: 2 }], 'the part that was not saved stays pending')
}

// --- validation -------------------------------------------------------------------------------------------------------------------
eq(pointsProblem('', 5), null, 'empty is fine')
eq(pointsProblem(' 5 ', 5), null, 'the full marks are fine')
eq(pointsProblem('0', 5), null, 'zero is fine')
eq(pointsProblem('5.5', 5), 'range', 'above the full marks')
eq(pointsProblem('-1', 5), 'range', 'negative')
eq(pointsProblem('abc', 5), 'format', 'not a number')
eq(pointsProblem('Infinity', 5), 'format', 'infinity is not a grade')
{
  const { state, base } = fresh()
  type(state, 'q1', 'a1', { points: '6' })
  eq(validateItems(buildItems(state, base), base), { code: 'points', question_id: 'q1', attempt_id: 'a1' }, 'points above max')
  type(state, 'q1', 'a1', { points: '5' })
  eq(validateItems(buildItems(state, base), base), null, 'valid')
  type(state, 'q1', 'a2', { feedback: 'x'.repeat(FEEDBACK_MAX) })
  eq(validateItems(buildItems(state, base), base), null, 'exactly the limit is fine')
  type(state, 'q1', 'a2', { feedback: 'x'.repeat(FEEDBACK_MAX + 1) })
  eq(validateItems(buildItems(state, base), base), { code: 'feedback', question_id: 'q1', attempt_id: 'a2' }, 'one over the limit')
  type(state, 'q1', 'a2', { feedback: '' })
  type(state, 'q1', 'a1', { points: 'x' })
  eq(validateItems(buildItems(state, base), base)?.code, 'points', 'NaN is reported, not dropped')
  eq(diffCell({ points: 'x', feedback: '' }, { points: null, feedback: null, max: 5 }).points !== undefined, true, 'a non-number stays in the diff')
}
// the limit counts characters, not UTF-16 units: an emoji is one
eq(charCount('😀'.repeat(10)), 10, 'code points')
eq(validateItems([{ attempt_id: 'a', question_id: 'q', feedback: '😀'.repeat(FEEDBACK_MAX) }], new Map([['q', new Map([['a', { points: null, feedback: null, max: 1 }]])]])), null, '500 emoji = 500 characters')

// --- chunking ---------------------------------------------------------------------------------------------------------------------
eq(chunkItems([], 3), [], 'no items, no calls')
eq(chunkItems([1, 2, 3, 4, 5, 6, 7], 3), [[1, 2, 3], [4, 5, 6], [7]], 'chunks of three')
eq(chunkItems(Array.from({ length: BATCH_MAX }, (_, i) => i)).length, 1, 'exactly 500 is one call')
eq(chunkItems(Array.from({ length: BATCH_MAX + 1 }, (_, i) => i)).map((c) => c.length), [BATCH_MAX, 1], '501 is two calls')
eq(chunkItems(Array.from({ length: 1234 }, (_, i) => i)).map((c) => c.length), [500, 500, 234], '1234 is three calls')

// --- the next question with work left -----------------------------------------------------------------------------------------------
eq(nextWork([{ pending: 1 }, { pending: 0 }, { pending: 2 }], 0), 2, 'skips questions with nothing waiting')
eq(nextWork([{ pending: 1 }, { pending: 0 }, { pending: 2 }], 2), 0, 'wraps around')
eq(nextWork([{ pending: 1 }, { pending: 0 }], 0), -1, 'only the current one has work: stay')
eq(nextWork([{ pending: 0 }, { pending: 0 }], 0), -1, 'nothing left')
eq(nextWork([], 0), -1, 'no questions')
eq(nextWork([{ pending: 3 }], 0), -1, 'a single question never moves')

// --- bulk tools ------------------------------------------------------------------------------------------------------------------------
{
  const { state, base } = fresh()
  eq(fillWaiting(state, base, 'q1', ['a1', 'a2'], 'full'), 2, 'both waiting answers get full marks')
  eq(buildItems(state, base).map((i) => i.points), [5, 5], 'full marks = the question max')
  eq(fillWaiting(state, base, 'q1', ['a1', 'a2'], 'zero'), 0, 'a box that holds a value is never overwritten, not even by an earlier bulk click')
  eq(buildItems(state, base).map((i) => i.points), [5, 5], 'so the grades stay as they were')
  type(state, 'q1', 'a1', { points: '' })
  type(state, 'q1', 'a2', { points: '' })
  eq(fillWaiting(state, base, 'q1', ['a1', 'a2'], 'zero'), 2, 'clearing the boxes first lets the other bulk tool fill them')
  eq(buildItems(state, base).map((i) => i.points), [0, 0], 'zero')
  eq(fillWaiting(state, base, 'q2', ['a1'], 'full'), 0, 'an already graded answer is never touched by a bulk tool')
  eq(state.get('q2')?.get('a1')?.points, '2', 'and keeps its grade')
  eq(fillWaiting(state, base, 'q9', ['x'], 'full'), 0, 'unknown question: nothing')
}
// a grade typed and not saved yet is not overwritten by "full marks for the rest"
{
  const { state, base } = fresh()
  type(state, 'q1', 'a1', { points: '3' })
  eq(fillWaiting(state, base, 'q1', ['a1', 'a2'], 'full'), 1, 'only the empty box is filled')
  eq([state.get('q1')?.get('a1')?.points, state.get('q1')?.get('a2')?.points], ['3', '5'], 'the typed 3 stays, the empty one gets full marks')
  type(state, 'q1', 'a2', { points: ' ' })
  eq(fillWaiting(state, base, 'q1', ['a1', 'a2'], 'zero'), 1, 'a box with only blanks counts as empty')
  eq([state.get('q1')?.get('a1')?.points, state.get('q1')?.get('a2')?.points], ['3', '0'], 'zero for the empty one only')
}

// --- an edit on an answer that left the screen stays on screen ----------------------------------------------------------------------
{
  const { state, base } = fresh()
  type(state, 'q2', 'a1', { points: '1' }) // a correction of a graded answer
  type(state, 'q1', 'a2', { points: '2' }) // a grade for a waiting one
  const previous = sheet()
  // "ungraded only" is switched on: the graded answer (q2/a1) is not listed any more; a colleague also graded q1/a2
  const next = sheet().map((q) => ({ ...q, answers: q.answers.filter((a) => a.points === null && a.attempt_id !== 'a2') }))
  eq(keepEdited(previous, next, state, base), 2, 'both edited answers are put back')
  eq(next.map((q) => q.answers.map((a) => a.attempt_id)), [['a1', 'a2'], ['a1'], ['a3']], 'each in its own question, after the ones the server listed')
  mergeSheet(state, base, next)
  eq(buildItems(state, base).map((i) => `${i.question_id}/${i.attempt_id}`), ['q1/a2', 'q2/a1'], 'and the save set is exactly what is on screen')
  // saved: the server copy now equals the boxes, so the next reload no longer keeps them
  markSaved(base, buildItems(state, base))
  const again = sheet().map((q) => ({ ...q, answers: q.answers.filter((a) => a.points === null && a.attempt_id !== 'a2') }))
  eq(keepEdited(next, again, state, base), 0, 'nothing edited is left to keep')
  eq(again.map((q) => q.answers.length), [1, 0, 1], 'so the graded ones disappear again')
}
{
  // typing a graded answer back to the server value drops it from the kept ones; untouched answers are never kept
  const { state, base } = fresh()
  type(state, 'q2', 'a1', { points: '1' })
  const previous = sheet()
  type(state, 'q2', 'a1', { points: '2' })
  const next = sheet().map((q) => ({ ...q, answers: q.answers.filter((a) => a.points === null) }))
  eq(keepEdited(previous, next, state, base), 0, 'back at the server value: no edit')
  eq(keepEdited(undefined, next, state, base), 0, 'the very first load has no previous sheet')
  type(state, 'q2', 'a1', { feedback: 'تعليق جديد' })
  eq(keepEdited(previous, next, state, base), 1, 'a comment edit alone counts as an edit too')
  eq(keepEdited(previous, next, state, base), 0, 'and an answer already listed is not added twice')
}

// --- the comment as the server keeps it ---------------------------------------------------------------------------------------------------
eq(cleanFeedback('  جيد  '), 'جيد', 'trimmed')
eq(cleanFeedback('سطر\r\nثان\tوتبويب'), 'سطر\nثان\tوتبويب', 'line breaks and tabs stay, CR goes')
eq(cleanFeedback('a\u0000b\u0007c\u001bd\u0085e'), 'abcde', 'control characters')
eq(cleanFeedback('\u202Egnirts\u202C'), 'gnirts', 'a bidi override')
eq(cleanFeedback('a\u2066b\u2069c\u200Bd\u200Ee\u200Ff\uFEFFg\u2060h'), 'abcdefgh', 'isolates, zero-width space, marks, BOM, word joiner')
eq(cleanFeedback('\u202E  \u200B\uFEFF'), '', 'nothing visible is left')
eq(cleanFeedback('\u200C\u200D \n'), '', 'joiners alone are invisible')
eq(cleanFeedback('می\u200Cخواهم'), 'می\u200Cخواهم', 'a joiner between letters stays')
eq(cleanFeedback(''), '', 'empty')
{
  // the counter and the limit use the cleaned text: hidden characters neither use the 500 up nor hide an empty comment
  eq(feedbackTooLong('\u200B' + 'x'.repeat(FEEDBACK_MAX) + '\u200B'), false, 'hidden characters do not count')
  eq(feedbackTooLong('x'.repeat(FEEDBACK_MAX + 1)), true, 'one over')
  const { state, base } = fresh()
  type(state, 'q1', 'a2', { feedback: '\u200B\u202E' })
  eq(buildItems(state, base), [], 'an invisible comment on a comment-free answer is no change')
  type(state, 'q2', 'a1', { feedback: '\u200B' })
  eq(buildItems(state, base).find((i) => i.question_id === 'q2'), { attempt_id: 'a1', question_id: 'q2', feedback: '' }, 'over a stored comment it means "clear"')
}

// --- the single-attempt page -----------------------------------------------------------------------------------------------------
eq(toAttemptBody([]), {}, 'nothing')
eq(toAttemptBody([{ attempt_id: 'a', question_id: 'q1', points: 2 }]), { grades: { q1: 2 } }, 'points only: no empty feedback object')
eq(toAttemptBody([{ attempt_id: 'a', question_id: 'q1', feedback: '' }]), { feedback: { q1: '' } }, 'a cleared comment is still sent')
eq(toAttemptBody([{ attempt_id: 'a', question_id: 'q1', points: 0, feedback: 'x' }, { attempt_id: 'a', question_id: 'q2', points: 1 }]), { grades: { q1: 0, q2: 1 }, feedback: { q1: 'x' } }, 'both')
{
  const items = [
    { id: 'q1', max: 5, correct: null, points: 0, feedback: null },
    { id: 'q2', max: 2, correct: false, points: 0, feedback: 'راجع الدرس' },
    { id: 'q3', max: 3, correct: true, points: 3 },
  ]
  const s = sheetFromAttempt('att', items)
  eq(s.map((q) => q.pending), [1, 0, 0], 'only the ungraded item is pending')
  const state: CellMap = new Map()
  const base: ServerMap = new Map()
  mergeSheet(state, base, s)
  eq(state.get('q2')?.get('att'), { points: '0', feedback: 'راجع الدرس' }, 'a zero grade is a grade, not an empty box')
  eq(state.get('q1')?.get('att'), { points: '', feedback: '' }, 'a pending item starts empty')
  type(state, 'q1', 'att', { points: '3' })
  type(state, 'q2', 'att', { points: '1' })
  eq(toAttemptBody(buildItems(state, base)), { grades: { q1: 3, q2: 1 } }, 'a new grade and a correction, nothing for the untouched item')
}
eq(countByQuestion([{ attempt_id: 'a', question_id: 'q1', points: 1 }, { attempt_id: 'b', question_id: 'q1', points: 1 }, { attempt_id: 'a', question_id: 'q2', points: 1 }]) .get('q1'), 2, 'count per question')

// --- the screens keep both maps reactive: edits made through the proxies must reach the readers --------------------------------------
{
  const state = reactive<CellMap>(new Map())
  const base = reactive<ServerMap>(new Map())
  const items = computed(() => buildItems(state, base))
  const waiting = computed(() => base.get('q1')?.get('a1')?.points === null)
  eq(items.value, [], 'nothing before the first load')
  eq(waiting.value, false, 'unknown before the first load')
  mergeSheet(state, base, sheet())
  eq(waiting.value, true, 'the first load reaches readers of the server copy')
  eq(items.value, [], 'a freshly loaded sheet has no changes')
  const cell = state.get('q1')?.get('a1')
  if (!cell) throw new Error('no cell')
  cell.points = '3'
  eq(items.value, [{ attempt_id: 'a1', question_id: 'q1', points: 3 }], 'typing in a box changes the computed list')
  fillWaiting(state, base, 'q1', ['a1', 'a2'], 'full')
  eq(items.value.map((i) => i.points), [3, 5], 'a bulk fill reaches the computed list (and leaves the typed 3 alone)')
  markSaved(base, items.value)
  eq(items.value, [], 'saving clears it')
  eq(waiting.value, false, 'and the answer is no longer waiting')
}

console.log('grade sheet tests ok')
