// What the confirmation dialog tells the student before an attempt starts: how long it lasts, when it will really end (the
// closing time is a hard end, so a long exam started late ends sooner than its duration says) and which attempt it is.

/** An exam without a duration still ends after this long (the server's cap for an open-ended attempt). */
export const OPEN_ENDED_MS = 24 * 3_600_000

export interface StartPreviewInput {
  /** the server's time now (epoch ms) */
  now: number
  durationMin: number | null | undefined
  closesAt: number | null | undefined
  attemptsUsed: number
  maxAttempts: number
}

export interface StartPreview {
  /** the exam has no time limit of its own */
  openEnded: boolean
  durationMin: number | null
  /** when the attempt will end (epoch ms); null when nothing ends it within the cap */
  endsAt: number | null
  /** it is the closing time, not the duration, that ends the attempt */
  limitedByClosing: boolean
  /** this attempt is number `attemptNo` of `attemptsMax` */
  attemptNo: number
  attemptsMax: number
}

export function startPreview(i: StartPreviewInput): StartPreview {
  const duration = i.durationMin && i.durationMin > 0 ? i.durationMin : null
  const byDuration = duration ? i.now + duration * 60_000 : null
  const cap = byDuration ?? i.now + OPEN_ENDED_MS
  const closes = typeof i.closesAt === 'number' && Number.isFinite(i.closesAt) ? i.closesAt : null
  const limitedByClosing = closes !== null && closes <= cap
  return {
    openEnded: duration === null,
    durationMin: duration,
    endsAt: limitedByClosing ? closes : byDuration,
    limitedByClosing,
    attemptNo: Math.max(0, i.attemptsUsed) + 1,
    attemptsMax: Math.max(1, i.maxAttempts),
  }
}
