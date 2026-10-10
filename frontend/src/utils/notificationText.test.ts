import { fillTemplate, notificationKey, renderNotification } from './notificationText.ts'

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
  n_score_changed_grade: 'Grader changed {title}: {old} to {new} of {total}.',
  n_new_enrollment: 'A student joined {subject}.',
  n_new_enrollment_many: '{count} students joined {subject}.',
  n_new_follower: 'A student follows you.',
  n_new_follower_many: '{count} students follow you.',
  n_submission_pending: 'A submission on {title} waits.',
  n_submission_pending_many: '{count} submissions on {title} wait.',
  n_new_review: 'New review ({rating} of 5).',
  n_new_review_course: 'New review ({rating} of 5) on {title}.',
  n_new_review_course_comment: 'New review ({rating} of 5) on {title}: {comment}',
  n_new_review_comment: 'New review of you ({rating} of 5): {comment}',
  n_live_updated: '{teacher} changed {title}, now {starts_at}.',
  n_content_auto_hidden: '{title} was hidden automatically.',
  n_feedback_added: 'Feedback on {title}.',
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

// --- a grader's correction says so (the key-correction wording has no cause) -------------------------------------------------
eq(notificationKey('score_changed', { title: 'T', old: 1, new: 2, total: 3 }), 'n_score_changed', 'no cause = answer-key correction')
eq(notificationKey('score_changed', { title: 'T', old: 1, new: 2, total: 3, cause: 'key' }), 'n_score_changed', 'any other cause keeps the key wording')
eq(notificationKey('score_changed', { title: 'T', old: 1, new: 2, total: 3, cause: 'grade' }), 'n_score_changed_grade', 'grade cause')
eq(renderNotification(lookup, 'score_changed', { title: 'Mid', old: 4, new: 6, total: 10, cause: 'grade' }), 'Grader changed Mid: 4 to 6 of 10.', 'grade wording')

// --- counted notices: singular for one (or no count), plural from two ----------------------------------------------------------
eq(renderNotification(lookup, 'new_enrollment', { subject: 'Math', count: 1 }), 'A student joined Math.', 'one student')
eq(renderNotification(lookup, 'new_enrollment', { subject: 'Math' }), 'A student joined Math.', 'no count: singular')
eq(renderNotification(lookup, 'new_enrollment', { subject: 'Math', count: 4 }), '4 students joined Math.', 'four students')
eq(renderNotification(lookup, 'new_enrollment', { subject: 'Math', count: '4' }), '4 students joined Math.', 'a numeric string counts')
eq(renderNotification(lookup, 'new_enrollment', { subject: 'Math', count: 0 }), 'A student joined Math.', 'zero is not plural')
eq(renderNotification(lookup, 'new_follower', { count: 2 }), '2 students follow you.', 'followers plural')
eq(renderNotification(lookup, 'new_follower', { count: 1 }), 'A student follows you.', 'follower singular')
eq(renderNotification(lookup, 'submission_pending', { title: 'Final', count: 3 }), '3 submissions on Final wait.', 'submissions plural')
eq(renderNotification(lookup, 'submission_pending', { title: 'Final', count: 1 }), 'A submission on Final waits.', 'submission singular')
// a language without the plural wording falls back to the singular one instead of printing the kind
eq(renderNotification((k) => (k === 'n_new_follower' ? 'follow {count}' : ''), 'new_follower', { count: 5 }), 'follow 5', 'missing plural falls back')
// the title is teacher/student text: dollar patterns and placeholders stay as written
eq(renderNotification(lookup, 'submission_pending', { title: "Q$& {count}", count: 3 }), "3 submissions on Q$& {count} wait.", 'title is data')

// --- reviews: the plain wording is the fallback ----------------------------------------------------------------------------------
eq(notificationKey('new_review', { rating: 5 }), 'n_new_review', 'old notice: no title, no comment')
eq(notificationKey('new_review', { rating: 5, title: '', comment: '' }), 'n_new_review', 'empty title and comment')
eq(notificationKey('new_review', { rating: 5, title: '  ', comment: '  ' }), 'n_new_review', 'blank is empty')
eq(notificationKey('new_review', { rating: 5, title: 'Algebra', comment: '' }), 'n_new_review_course', 'course, no comment')
eq(notificationKey('new_review', { rating: 5, title: 'Algebra', comment: 'Great' }), 'n_new_review_course_comment', 'course and comment')
eq(notificationKey('new_review', { rating: 5, title: '', comment: 'Great' }), 'n_new_review_comment', 'teacher review with a comment')
eq(renderNotification(lookup, 'new_review', { rating: 4 }), 'New review (4 of 5).', 'plain review')
eq(renderNotification(lookup, 'new_review', { rating: 4, title: 'Algebra', comment: '' }), 'New review (4 of 5) on Algebra.', 'review of a course')
eq(renderNotification(lookup, 'new_review', { rating: 4, title: 'Algebra', comment: "Loved $& it" }), "New review (4 of 5) on Algebra: Loved $& it", 'comment is data')
eq(renderNotification(lookup, 'new_review', { rating: 2, title: '', comment: 'Slow' }), 'New review of you (2 of 5): Slow', 'review of the teacher')

// --- the new one-off kinds ---------------------------------------------------------------------------------------------------------------
eq(renderNotification(lookup, 'live_updated', { title: 'Lab', teacher: 'Sara', starts_at: 5 }, (ms) => `T${ms}`), 'Sara changed Lab, now T5.', 'live update formats the time')
eq(renderNotification(lookup, 'content_auto_hidden', { title: 'Post' }), 'Post was hidden automatically.', 'auto hidden')
eq(renderNotification(lookup, 'feedback_added', { title: 'Quiz' }), 'Feedback on Quiz.', 'feedback added')

// --- fillTemplate on its own ---------------------------------------------------------------------------------------------------------------
eq(fillTemplate('{a}-{b}-{c}', { a: 1, b: 'x', c: null }), '1-x-', 'null prints nothing')
eq(fillTemplate('{a} {missing}', { a: '{b}', b: 'no' }), '{b} {missing}', 'one pass, missing stays')
eq(fillTemplate('At {done_at}', { done_at: 9 }, (ms) => `T${ms}`), 'At T9', '*_at goes through fmtTime')
eq(fillTemplate('At {done_at}', { done_at: 'soon' }, (ms) => `T${ms}`), 'At soon', 'a text *_at is printed as is')
