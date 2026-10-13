// The pure rules of the exam-taking page (TakeAssessmentView): which answers still have to be sent, how long to wait before
// sending a failed hand-in again, when the "N minutes left" announcements fire, when leaving the page needs a confirmation,
// and the arrow-key movement of a single-choice group. Kept out of the view so a plain script can test them.

import { isAttemptOverCode } from './attemptDrafts'

// ── answers ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

export const isAnswered = (v: string | null | undefined): boolean => (v ?? '').trim() !== ''

type Answers = Readonly<Record<string, string>>

/**
 * What differs between the answers on screen and what the server is known to hold: every question whose value changed, with
 * `''` for one that had an answer and was cleared (the server's merge removes the key). Questions that are empty on both
 * sides are not mentioned. The autosave sends only this, so two devices saving different questions never erase each other.
 */
export function changedAnswers(current: Answers, saved: Answers): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [id, v] of Object.entries(current)) {
    if ((saved[id] ?? '') !== v && (isAnswered(v) || isAnswered(saved[id]))) out[id] = isAnswered(v) ? v : ''
  }
  for (const id of Object.keys(saved)) {
    if (!(id in current) && isAnswered(saved[id])) out[id] = ''
  }
  return out
}

/** The server's copy after it accepted `sent` (a merge: an empty value removes the question). */
export function applySaved(saved: Answers, sent: Answers): Record<string, string> {
  const out: Record<string, string> = { ...saved }
  for (const [id, v] of Object.entries(sent)) {
    if (isAnswered(v)) out[id] = v
    else delete out[id]
  }
  return out
}

/** How many questions of the page have an answer. */
export function answeredCount(questions: readonly { id: string }[], answers: Answers): number {
  return questions.reduce((n, q) => n + (isAnswered(answers[q.id]) ? 1 : 0), 0)
}

/** A browser lets a request with `keepalive` carry at most 64 KiB; stay below it, otherwise send the normal way. */
export const KEEPALIVE_MAX_BYTES = 60_000
export function fitsKeepalive(body: string): boolean {
  const bytes = typeof TextEncoder !== 'undefined' ? new TextEncoder().encode(body).length : body.length * 3
  return bytes <= KEEPALIVE_MAX_BYTES
}

// ── a failed hand-in ─────────────────────────────────────────────────────────────────────────────────────────────────────

/** Pause before the n-th retry (n counts the failures so far, from 1): 2, 4, 8, 16 seconds, then 30 at most. Never a tight loop. */
export function retryDelayMs(failures: number): number {
  const n = Math.max(1, Math.floor(failures))
  return n <= 4 ? 2000 * 2 ** (n - 1) : 30_000
}

/**
 * Whether a failed submit is worth sending again by itself: the connection dropped, the server hiccuped, or it asked us to
 * slow down. A definite answer ("not found", "forbidden", a lost session) is not retried - and "the attempt is over" is not
 * an error at all: the page moves on to the result.
 */
export function isRetryableSubmitError(e: { code?: string | null; status?: number | null } | null | undefined): boolean {
  if (!e) return false
  if (isAttemptOverCode(e.code)) return false
  if (e.code === 'network' || e.code === 'rate_limited') return true
  const s = e.status ?? 0
  return s === 0 || s === 408 || s === 429 || s >= 500
}

// ── time-left announcements ──────────────────────────────────────────────────────────────────────────────────────────────

/** The minutes at which the page tells a screen-reader user how much time is left (each once). */
export const TIME_ANNOUNCE_MINUTES = [10, 5, 1] as const

/**
 * Which announcement is due at `remainingSec`, given those already made. The smallest threshold that has been crossed wins,
 * and every larger one counts as made (opening the page with four minutes left says "5 minutes", not also "10 minutes").
 * Nothing is announced at 0 (the time is up message says so).
 */
export function announceDue(remainingSec: number, announced: ReadonlySet<number>): { minutes: number | null; announced: Set<number> } {
  const next = new Set(announced)
  if (remainingSec <= 0) return { minutes: null, announced: next }
  const crossed = TIME_ANNOUNCE_MINUTES.filter((m) => remainingSec <= m * 60)
  if (!crossed.length) return { minutes: null, announced: next }
  const smallest = Math.min(...crossed)
  for (const m of crossed) next.add(m)
  return { minutes: announced.has(smallest) ? null : smallest, announced: next }
}

/**
 * The announcements that are already behind the page when it first reads the time: every threshold the time left is at or
 * below. An 8-minute quiz must not open with "10 minutes left", and a reload at four minutes must not say "5 minutes left"
 * again - what was due before the page opened was said (or not needed) already; only the thresholds still ahead are announced.
 * Used again when the end moves later (the closing time was extended), so the longer time gets its announcements afresh.
 */
export function seedAnnounced(remainingSec: number): Set<number> {
  return new Set(TIME_ANNOUNCE_MINUTES.filter((m) => remainingSec <= m * 60))
}

// ── the clock ───────────────────────────────────────────────────────────────────────────────────────────────────────────

/**
 * Whether the page shows a countdown: the attempt has a duration, or the closing time cuts it short. An open-ended exam
 * with no near closing time gets the server's 24-hour cap and no timer.
 */
export function isTimedAttempt(a: { started_at: number; ends_at: number; limited_by_closing?: boolean | null }): boolean {
  return a.limited_by_closing === true || a.ends_at - a.started_at < 24 * 3_600_000
}

/**
 * Whether the device's wall clock stepped (set by hand, a network time correction, waking from sleep) between two ticks: the
 * wall clock advanced by a different amount than the monotonic clock (`performance.now()`). A step of more than this much
 * means the displayed time cannot be trusted until the server has been asked again.
 */
export const CLOCK_STEP_TOLERANCE_MS = 5000
export function clockStepped(wallDeltaMs: number, monotonicDeltaMs: number): boolean {
  return Math.abs(wallDeltaMs - monotonicDeltaMs) > CLOCK_STEP_TOLERANCE_MS
}

// ── leaving the page ─────────────────────────────────────────────────────────────────────────────────────────────────────

/**
 * Whether a navigation away from the take page needs the "leave the exam?" confirmation: an attempt is open (not yet handed
 * in, not settled by the server), the person is signed in (a lost session must reach the sign-in page unhindered), and the
 * move really leaves the page.
 */
export function shouldAskBeforeLeaving(i: { active: boolean; loggedIn: boolean; toPath: string; fromPath: string }): boolean {
  return i.active && i.loggedIn && i.toPath !== i.fromPath
}

// ── a single-choice group ────────────────────────────────────────────────────────────────────────────────────────────────

/**
 * The index the arrow keys move a radio group to (wrapping), or null for any other key. In a right-to-left page the
 * horizontal arrows are mirrored: the next option is to the left. Home / End jump to the ends.
 */
export function radioMove(key: string, index: number, count: number, rtl: boolean): number | null {
  if (count <= 0) return null
  const wrap = (n: number) => (n + count) % count
  switch (key) {
    case 'ArrowDown':
      return wrap(index + 1)
    case 'ArrowUp':
      return wrap(index - 1)
    case 'ArrowRight':
      return wrap(index + (rtl ? -1 : 1))
    case 'ArrowLeft':
      return wrap(index + (rtl ? 1 : -1))
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}
