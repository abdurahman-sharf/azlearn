import { notificationKey, renderNotification } from './notificationText.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const templates: Record<string, string> = {
  n_teaching_approved: 'Approved for {subject}.',
  n_teaching_approved_reason: 'Approved for {subject}. Note: {reason}',
  n_teaching_revoked: 'Withdrawn from {subject}; {hidden} hidden.',
  n_teaching_revoked_reason: 'Withdrawn from {subject}; {hidden} hidden. Note: {reason}',
  n_teaching_revoked_none: 'Withdrawn from {subject}.',
  n_teaching_revoked_none_reason: 'Withdrawn from {subject}. Note: {reason}',
  n_exam_closing: 'Closes {closes_at}.',
  n_score_changed: 'Score on {title} went from {old} to {new} out of {total}.',
}
const lookup = (k: string): string => templates[k] ?? ''

// --- keys -----------------------------------------------------------------------------------------------------------
eq(notificationKey('teaching_approved', {}), 'n_teaching_approved', 'plain kind')
eq(notificationKey('teaching_revoked', { hidden: 5 }), 'n_teaching_revoked', 'items hidden: the counted wording')
eq(notificationKey('teaching_revoked', { hidden: 0 }), 'n_teaching_revoked_none', 'nothing hidden: no count')
eq(notificationKey('teaching_revoked', {}), 'n_teaching_revoked_none', 'no count at all')
eq(notificationKey('teaching_revoked', { hidden: null }), 'n_teaching_revoked_none', 'null count')
eq(notificationKey('teaching_revoked', { hidden: '3' }), 'n_teaching_revoked', 'a numeric string still counts')

// --- rendering ------------------------------------------------------------------------------------------------------
eq(renderNotification(lookup, 'teaching_revoked', { subject: 'Math', hidden: 5 }), 'Withdrawn from Math; 5 hidden.', 'counted')
eq(renderNotification(lookup, 'teaching_revoked', { subject: 'Math', hidden: 0 }), 'Withdrawn from Math.', 'never "0 hidden"')
eq(renderNotification(lookup, 'teaching_revoked', { subject: 'Math', hidden: 0, reason: 'late' }), 'Withdrawn from Math. Note: late', 'none + note')
eq(renderNotification(lookup, 'teaching_revoked', { subject: 'Math', hidden: 2, reason: 'late' }), 'Withdrawn from Math; 2 hidden. Note: late', 'counted + note')
eq(renderNotification(lookup, 'teaching_approved', { subject: 'Math', reason: '   ' }), 'Approved for Math.', 'a blank note uses the plain wording')
eq(renderNotification(lookup, 'unknown_kind', { a: 1 }), 'unknown_kind', 'no template: the kind itself')
eq(renderNotification(lookup, 'exam_closing', { closes_at: 1000 }, (ms) => `T${ms}`), 'Closes T1000.', 'timestamps go through fmtTime')
eq(renderNotification(lookup, 'exam_closing', { closes_at: 1000 }), 'Closes 1000.', 'and print raw without it')

// --- an admin's free text is data, never a replacement pattern -------------------------------------------------------------
for (const note of ["cost $& and $' x", 'a $` b', 'price $$5', '$1 and $<name>', '{subject}']) {
  eq(renderNotification(lookup, 'teaching_approved', { subject: 'Math', reason: note }), `Approved for Math. Note: ${note}`, `note ${note}`)
}
// the subject name is data as well
eq(renderNotification(lookup, 'teaching_approved', { subject: "Q$'A" }), "Approved for Q$'A.", 'subject with a dollar sign')
// a note that contains another placeholder is not substituted a second time, whatever order the data keys come in
eq(renderNotification(lookup, 'teaching_approved', { reason: '{subject}', subject: 'Math' }), 'Approved for Math. Note: {subject}', 'no second pass')
// a placeholder with no data stays visible rather than turning into "undefined"
eq(renderNotification(lookup, 'teaching_approved', {}), 'Approved for {subject}.', 'missing data')

// an answer-key correction tells the student the old and new score; `new` is an ordinary placeholder name, the title is data
eq(renderNotification(lookup, 'score_changed', { title: 'Midterm', old: 7, new: 9, total: 10 }), 'Score on Midterm went from 7 to 9 out of 10.', 'score changed')
eq(renderNotification(lookup, 'score_changed', { title: "Q$&A {new}", old: 0, new: 2.5, total: 4 }), "Score on Q$&A {new} went from 0 to 2.5 out of 4.", 'a title with a dollar sign and a placeholder is printed as written')
