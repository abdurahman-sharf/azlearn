// Rendering of a stored notification (`kind` + `data` JSON) into the viewer's language. Kept free of the i18n store so
// it can be tested as a plain script; `formatNotification` in `i18n/platform.ts` is the thin wrapper the UI calls.

export type NotificationData = Record<string, string | number | null>

/** Kinds whose `data.count` folds several events into one notice ("3 students joined"): more than one uses the `_many` wording. */
const COUNTED = new Set(['new_enrollment', 'new_follower', 'submission_pending'])

const plain = (v: string | number | null | undefined): string => (typeof v === 'string' ? v.trim() : '')

/**
 * The message key of a notification, without its `_reason` variant.
 *
 * `teaching_revoked` normally says how many of the teacher's items just became hidden; with nothing hidden (0 or no
 * count at all) a "0 of your items are now hidden" would only alarm, so the wording without the count is used.
 * `score_changed` says why the score moved: after a grader's correction (`cause: 'grade'`) or an answer-key correction
 * (no cause). Counted kinds switch to a plural wording from the second event on, and a review is described by its
 * course title and a snippet of the comment when it carries them (older notices carry neither: the plain wording).
 */
export function notificationKey(kind: string, data: NotificationData): string {
  if (kind === 'teaching_revoked' && !(Number(data.hidden) > 0)) return 'n_teaching_revoked_none'
  if (kind === 'score_changed' && data.cause === 'grade') return 'n_score_changed_grade'
  if (COUNTED.has(kind) && Number(data.count) > 1) return `n_${kind}_many`
  if (kind === 'new_review') {
    const titled = plain(data.title) !== ''
    const commented = plain(data.comment) !== ''
    if (titled && commented) return 'n_new_review_course_comment'
    if (titled) return 'n_new_review_course'
    if (commented) return 'n_new_review_comment'
  }
  return `n_${kind}`
}

/**
 * Puts `data` into the `{name}` placeholders of a template in ONE pass: the text put in for one placeholder is never
 * scanned again (a name that contains `{subject}` stays as written), and a replacer function keeps `$&`, `$'`, `` $` ``
 * and `$$` literal. A placeholder with no data stays visible rather than turning into "undefined". `fmtTime` formats
 * numeric `*_at` fields (epoch ms) in the viewer's language.
 */
export function fillTemplate(text: string, data: NotificationData, fmtTime?: (ms: number) => string): string {
  return text.replace(/\{([A-Za-z0-9_]+)\}/g, (whole, name: string) => {
    if (!Object.prototype.hasOwnProperty.call(data, name)) return whole
    const v = data[name]
    return fmtTime && name.endsWith('_at') && typeof v === 'number' ? fmtTime(v) : String(v ?? '')
  })
}

/**
 * `lookup` returns the localized template for a message key, or `''` when there is none (then the raw `kind` is shown).
 * When the data carries an admin's note and the kind has a `_reason` wording, that one (which includes the note) wins.
 * A variant wording (`_many`, review variants...) missing from a language falls back to the plain `n_<kind>` one.
 * `fmtTime` formats numeric `*_at` fields (epoch ms) in the viewer's language.
 */
export function renderNotification(
  lookup: (key: string) => string,
  kind: string,
  data: NotificationData,
  fmtTime?: (ms: number) => string,
): string {
  const key = notificationKey(kind, data)
  const note = data.reason
  const withNote = typeof note === 'string' && note.trim() !== '' ? lookup(`${key}_reason`) : ''
  const text = withNote || lookup(key) || lookup(`n_${kind}`)
  if (!text) return kind
  return fillTemplate(text, data, fmtTime)
}
