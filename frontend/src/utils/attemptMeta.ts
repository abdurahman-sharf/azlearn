// The small facts the result page shows beside the score: how long the attempt took and which attempt it is.

/** `m:ss`, or `h:mm:ss` from one hour on. Digits only, so the page wraps it in a left-to-right span. `null` when unknown. */
export function formatDuration(totalSec: number | null | undefined): string | null {
  if (typeof totalSec !== 'number' || !Number.isFinite(totalSec) || totalSec < 0) return null
  const s = Math.floor(totalSec)
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const two = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${two(m)}:${two(sec)}` : `${m}:${two(sec)}`
}

/** The attempt meta block has something to show (an older server sends none of these fields). */
export function hasAttemptMeta(r: { attempt_no?: number | null; is_best?: boolean | null; duration_sec?: number | null; submitted_at?: number | null }): boolean {
  return typeof r.attempt_no === 'number' || r.is_best === true || typeof r.duration_sec === 'number' || typeof r.submitted_at === 'number'
}

/** A difficulty word's translation key (`bank_d_easy`...), or null for a value the app does not know. */
export function difficultyKey(d: string | null | undefined): `bank_d_${'easy' | 'medium' | 'hard'}` | null {
  return d === 'easy' || d === 'medium' || d === 'hard' ? `bank_d_${d}` : null
}

/**
 * What the withheld-result notice says about WHEN it appears: `at` = a release moment that is still ahead (show it), `soon` = it
 * has been reached by the clock but the server withholds the result for the grace after it (the last save may still land),
 * `none` = no moment is known. `releaseAt` is the closing time the server sends; `now` the server's clock (epoch ms).
 */
export function releaseNotice(releaseAt: number | null | undefined, now: number): 'at' | 'soon' | 'none' {
  if (typeof releaseAt !== 'number' || !Number.isFinite(releaseAt)) return 'none'
  return releaseAt <= now ? 'soon' : 'at'
}
