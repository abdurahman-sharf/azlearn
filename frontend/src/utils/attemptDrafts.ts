// The exam-taking page keeps a local copy of the answers typed so far (`exameow-attempt-<attempt id>`), so a reload or a
// lost connection never loses them. The copy is NOT protected by the account: on a shared computer it must go when the
// person signs out, and the page removes its own copy once the attempt is over (submitted, or settled by the server).

export const ATTEMPT_DRAFT_PREFIX = 'exameow-attempt-'

export const attemptDraftKey = (attemptId: string) => `${ATTEMPT_DRAFT_PREFIX}${attemptId}`
/** The questions the student marked for review ("come back to this"). Starts with the draft prefix, so signing out removes it with the drafts. */
export const attemptMarksKey = (attemptId: string) => `${ATTEMPT_DRAFT_PREFIX}${attemptId}-marks`

/** The answers a stored draft holds: only string values of a plain object count; anything else (corrupt text, an array) is no draft at all. */
export function parseDraft(raw: string | null | undefined): Record<string, string> {
  if (!raw) return {}
  try {
    const v: unknown = JSON.parse(raw)
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {}
    const out: Record<string, string> = {}
    for (const [k, x] of Object.entries(v)) if (typeof x === 'string') out[k] = x
    return out
  } catch {
    return {}
  }
}

type Store = Pick<Storage, 'length' | 'key' | 'removeItem'>

/** Every stored exam draft (any attempt). Never throws: blocked storage holds nothing we can read. */
export function attemptDraftKeys(storage: Pick<Storage, 'length' | 'key'> | null | undefined): string[] {
  const out: string[] = []
  if (!storage) return out
  try {
    for (let i = 0; i < storage.length; i++) {
      const k = storage.key(i)
      if (k && k.startsWith(ATTEMPT_DRAFT_PREFIX)) out.push(k)
    }
  } catch { /* storage unavailable */ }
  return out
}

/** Removes every exam draft; returns how many were removed. The keys are collected first (removing shifts the indexes). */
export function removeAttemptDrafts(storage: Store | null | undefined): number {
  let n = 0
  for (const k of attemptDraftKeys(storage)) {
    try {
      storage!.removeItem(k)
      n++
    } catch { /* storage unavailable */ }
  }
  return n
}

/** The server has already ended the attempt: it was submitted somewhere else, or settled when its time (plus grace) ran out. */
export const isAttemptOverCode = (code: string | null | undefined): boolean => code === 'time_expired' || code === 'already_submitted'

/** Pause between two automatic submissions that failed (offline, server error): not one request per second. */
export const AUTO_SUBMIT_BACKOFF_MS = 5000

/**
 * Whether the page should hand the attempt in by itself now: the timed attempt ran out, nothing is being sent, the server
 * has not said the attempt is over, and the last automatic try (if any) is not too recent.
 */
export function autoSubmitDue(i: { timed: boolean; remainingSec: number; submitting: boolean; over: boolean; now: number; lastTryAt: number }): boolean {
  return i.timed && i.remainingSec <= 0 && !i.submitting && !i.over && i.now - i.lastTryAt >= AUTO_SUBMIT_BACKOFF_MS
}
