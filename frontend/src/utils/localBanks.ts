// The question banks and generated questions kept in this browser (localStorage). They are NOT protected by the
// platform account: whoever opens the same browser profile can read them, which matters on a shared computer. Sign-out
// therefore offers to wipe them - but only when there is something to wipe (see composables/useSignOut.ts).

/** Keys that hold question content a signed-out person should not be able to read on a shared device. */
export const LOCAL_QUESTION_KEYS = ['exameow-banks', 'exameow-questions'] as const
/** Everything the "clear" choice removes: the banks, the last generated questions, their source name and the saved practice run (it embeds question copies). */
export const LOCAL_QUESTION_CLEAR_KEYS = ['exameow-banks', 'exameow-questions', 'exameow-sourcefile', 'exameow-practice-session'] as const

type Reader = Pick<Storage, 'getItem'>

/** True when `raw` holds something other than an empty list / empty text. Unparseable but non-empty text counts as content (when in doubt, ask). */
function hasContent(raw: string | null): boolean {
  if (raw === null) return false
  const t = raw.trim()
  if (!t || t === '[]' || t === '{}' || t === 'null') return false
  try {
    const v: unknown = JSON.parse(t)
    return Array.isArray(v) ? v.length > 0 : v !== null && v !== undefined
  } catch {
    return true
  }
}

/** Whether any question bank or generated question is stored. Never throws (storage can be blocked). */
export function hasLocalQuestions(storage: Reader | null | undefined): boolean {
  if (!storage) return false
  for (const key of LOCAL_QUESTION_KEYS) {
    try {
      if (hasContent(storage.getItem(key))) return true
    } catch {
      /* unreadable storage: nothing we can offer to clear */
    }
  }
  return false
}
