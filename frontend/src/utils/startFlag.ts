// Opening the take page used to start an attempt by itself. Now starting goes through the confirmation dialog on the exam
// page, which leaves a flag (`exameow-start-<assessment id>` in sessionStorage = the time of the confirmation); the take page
// starts a NEW attempt only while that flag is fresh. Resuming a running attempt never needs it. A copied link, a reload long
// after the start, or the browser's back button therefore never burns an attempt.

export const START_FLAG_PREFIX = 'exameow-start-'
/** How long a confirmation stays good: long enough for a slow page load, short enough that a stale tab cannot start anything. */
export const START_FLAG_MAX_AGE_MS = 120_000
/** A flag stamped slightly in the future (clock adjusted between the two pages) is still fresh. */
const FUTURE_TOLERANCE_MS = 5000

export const startFlagKey = (assessmentId: string) => `${START_FLAG_PREFIX}${assessmentId}`

type KV = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

/** Where the flag lives when sessionStorage is blocked (private windows, strict settings): memory survives the in-app navigation. */
const memory = new Map<string, number>()

function session(): KV | null {
  try {
    return typeof sessionStorage === 'undefined' ? null : sessionStorage
  } catch {
    return null
  }
}

export interface StartFlag {
  /** the confirmation is recent enough to start a new attempt */
  valid: boolean
  /** the stamp that was stored (null when there is none or it is unreadable) */
  stamp: number | null
}

function parse(raw: string | null | undefined): number | null {
  if (!raw) return null
  const n = Number(raw)
  return Number.isFinite(n) && n > 0 ? n : null
}

/** Leaves the confirmation for the take page. Never throws. */
export function setStartFlag(assessmentId: string, now: number = Date.now(), storage: KV | null | undefined = session()): void {
  const key = startFlagKey(assessmentId)
  memory.set(key, now)
  try {
    storage?.setItem(key, String(now))
  } catch {
    /* blocked storage: the in-memory copy carries it */
  }
}

/** Reads the flag without removing it. */
export function readStartFlag(assessmentId: string, now: number = Date.now(), storage: KV | null | undefined = session()): StartFlag {
  const key = startFlagKey(assessmentId)
  let stamp: number | null = null
  try {
    stamp = parse(storage?.getItem(key))
  } catch {
    /* blocked storage */
  }
  if (stamp === null) stamp = memory.get(key) ?? null
  if (stamp === null) return { valid: false, stamp: null }
  const age = now - stamp
  return { valid: age >= -FUTURE_TOLERANCE_MS && age < START_FLAG_MAX_AGE_MS, stamp }
}

export function clearStartFlag(assessmentId: string, storage: KV | null | undefined = session()): void {
  const key = startFlagKey(assessmentId)
  memory.delete(key)
  try {
    storage?.removeItem(key)
  } catch {
    /* blocked storage */
  }
}

/** Reads and removes the flag (a stale one is removed too). Returns whether it was fresh. */
export function consumeStartFlag(assessmentId: string, now: number = Date.now(), storage: KV | null | undefined = session()): boolean {
  const f = readStartFlag(assessmentId, now, storage)
  if (f.stamp !== null) clearStartFlag(assessmentId, storage)
  return f.valid
}
