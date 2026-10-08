// Rendering of a stored notification (`kind` + `data` JSON) into the viewer's language. Kept free of the i18n store so
// it can be tested as a plain script; `formatNotification` in `i18n/platform.ts` is the thin wrapper the UI calls.

export type NotificationData = Record<string, string | number | null>

/**
 * The message key of a notification, without its `_reason` variant.
 *
 * `teaching_revoked` normally says how many of the teacher's items just became hidden; with nothing hidden (0 or no
 * count at all) a "0 of your items are now hidden" would only alarm, so the wording without the count is used.
 */
export function notificationKey(kind: string, data: NotificationData): string {
  if (kind === 'teaching_revoked' && !(Number(data.hidden) > 0)) return 'n_teaching_revoked_none'
  return `n_${kind}`
}

/**
 * `lookup` returns the localized template for a message key, or `''` when there is none (then the raw `kind` is shown).
 * When the data carries an admin's note and the kind has a `_reason` wording, that one (which includes the note) wins.
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
  const text = withNote || lookup(key)
  if (!text) return kind
  // One pass over the template: the text put in for one placeholder is never scanned again (an admin's note that
  // contains `{subject}` stays as written), and a replacer function keeps `$&`, `$'`, `` $` `` and `$$` literal.
  return text.replace(/\{([A-Za-z0-9_]+)\}/g, (whole, name: string) => {
    if (!Object.prototype.hasOwnProperty.call(data, name)) return whole
    const v = data[name]
    return fmtTime && name.endsWith('_at') && typeof v === 'number' ? fmtTime(v) : String(v ?? '')
  })
}
