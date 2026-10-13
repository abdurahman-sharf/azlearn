// The exam timer must not trust the device clock: a phone set five minutes fast or slow would show the wrong time left (and
// auto-submit early or late). The server sends its own time (`server_now`, epoch ms) with the start of an attempt and with
// every autosave reply; the page measures how far its clock is from the server's and shows the time left on the server's
// clock. Kept free of Vue so a plain script can test it.

/**
 * The offset to ADD to this device's clock to get the server's: the server stamped `serverNow` somewhere between the
 * request leaving (`before`) and the reply arriving (`after`, both on this device), so the best guess is the middle of the
 * round trip.
 */
export function clockOffset(serverNow: number, before: number, after: number): number {
  return serverNow - (before + after) / 2
}

export interface ClockSample {
  /** add to the device clock to get the server clock */
  offset: number
  /** round-trip time of the request this sample comes from: the smaller, the more exact the offset */
  rtt: number
  /** device time when the reply arrived */
  at: number
}

/** A sample older than this is replaced by a new one of reasonable quality (the device clock drifts, or the person changed it). */
export const CLOCK_SAMPLE_MAX_AGE_MS = 5 * 60_000
/**
 * "Reasonable quality" for replacing an AGED sample: a round trip no longer than twice the kept one's (but never stricter
 * than this). A single slow reply after a quiet spell on a flaky link says little about the offset - it is uncertain by up
 * to half its round trip - so it must not push the displayed deadline around.
 */
export const CLOCK_STALE_REPLACE_RTT_MS = 1500
/** A new sample that disagrees with the kept one by more than this is trusted if its round trip was short. */
export const CLOCK_JUMP_MS = 2000
export const CLOCK_JUMP_MAX_RTT_MS = 3000

export class ServerClock {
  private best: ClockSample | null = null

  constructor(private readonly deviceNow: () => number = () => Date.now()) {}

  /**
   * Takes a measurement. Returns whether it was adopted: the first one always, a later one when its round trip is not
   * longer than the kept one's (less uncertainty), when the kept one is old AND the new round trip is reasonable (at most
   * twice the kept one's, 1.5 s at least), when the device clock went backwards, or when it shows the device clock jumped
   * (a short round trip that disagrees by seconds). Nonsense (not a number, negative round trip) is ignored.
   */
  sync(serverNow: number | null | undefined, before: number, after: number): boolean {
    if (typeof serverNow !== 'number' || !Number.isFinite(serverNow) || !Number.isFinite(before) || !Number.isFinite(after)) return false
    const rtt = after - before
    if (rtt < 0) return false
    const offset = clockOffset(serverNow, before, after)
    const b = this.best
    const adopt =
      !b ||
      rtt <= b.rtt ||
      after < b.at ||
      (after - b.at > CLOCK_SAMPLE_MAX_AGE_MS && rtt <= Math.max(b.rtt * 2, CLOCK_STALE_REPLACE_RTT_MS)) ||
      (Math.abs(offset - b.offset) > CLOCK_JUMP_MS && rtt <= CLOCK_JUMP_MAX_RTT_MS)
    if (adopt) this.best = { offset, rtt, at: after }
    return adopt
  }

  /** The current offset (0 until a measurement was taken). */
  get offset(): number {
    return this.best?.offset ?? 0
  }

  /** Whether a measurement was taken (before that `now()` is the device clock). */
  get synced(): boolean {
    return this.best !== null
  }

  /** The server's current time (epoch ms), by this device's clock plus the measured offset. */
  now(): number {
    return this.deviceNow() + this.offset
  }

  reset(): void {
    this.best = null
  }
}

/** The page-wide clock: the exam page and the take page share it, so the take page starts with the offset already known. */
export const examClock = new ServerClock()

/** Whole seconds left until `endsAt` (epoch ms on the server's clock) at server time `now`, never negative. */
export function secondsLeft(endsAt: number, now: number): number {
  return Math.max(0, Math.floor((endsAt - now) / 1000))
}

/** `MM:SS` for a countdown (minutes may exceed 59: a 90-minute exam shows 90:00). */
export function clockText(totalSec: number): string {
  const s = Math.max(0, Math.floor(totalSec))
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}
