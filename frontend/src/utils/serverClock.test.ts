import { CLOCK_JUMP_MS, CLOCK_SAMPLE_MAX_AGE_MS, CLOCK_STALE_REPLACE_RTT_MS, ServerClock, clockOffset, clockText, secondsLeft } from './serverClock.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- the offset is measured at the middle of the round trip ------------------------------------------------------------------
eq(clockOffset(10_000, 9_000, 9_000), 1000, 'no latency: the server is one second ahead')
eq(clockOffset(10_000, 9_000, 11_000), 0, 'a two-second round trip: the server stamped at the middle, the clocks agree')
eq(clockOffset(5_000, 9_000, 9_400), -4200, 'a device clock fast by 4.2 s')

// --- a clock that follows the server -------------------------------------------------------------------------------------------
let device = 1_000_000
const c = new ServerClock(() => device)
eq(c.now(), 1_000_000, 'before any measurement it is the device clock')
eq(c.synced, false, 'not synced yet')
eq(c.sync(1_300_020, 1_000_000, 1_000_040), true, 'the first sample is always adopted')
eq(c.offset, 300_000, 'a device five minutes slow')
device = 1_010_000
eq(c.now(), 1_310_000, 'the server time moves with the device clock')
eq(secondsLeft(1_400_000, c.now()), 90, 'time left is measured on the server clock')

// a slower round trip does not replace a more exact sample
eq(c.sync(1_310_500, 1_009_000, 1_010_000), false, 'a longer round trip is ignored')
eq(c.offset, 300_000, 'offset kept')
// a faster (or equal) one does
eq(c.sync(1_310_010, 1_010_000, 1_010_020), true, 'a shorter round trip is adopted')
eq(c.offset, 1_310_010 - 1_010_010, 'new offset')

// garbage is ignored
const g = new ServerClock(() => 5)
eq(g.sync(undefined, 1, 2), false, 'no server time')
eq(g.sync(Number.NaN, 1, 2), false, 'NaN')
eq(g.sync(10, 5, 4), false, 'negative round trip')
eq(g.synced, false, 'still not synced')

// an old sample is replaced by a new one of reasonable quality, not by any slow reply (the server stamps are consistent with
// the same offset throughout, so no clock "jump" is involved)
const o = new ServerClock(() => 0)
o.sync(1000, 0, 10) // offset 995
const late = 10 + CLOCK_SAMPLE_MAX_AGE_MS
const reply = (before: number, rttMs: number, offset = 995) => [before + rttMs / 2 + offset, before, before + rttMs] as const
eq(o.sync(...reply(late + 1, 3000)), false, 'a stale sample is NOT replaced by a 3-second round trip (its error is up to 1.5 s)')
eq(o.offset, 995, 'the kept offset stands')
eq(o.sync(...reply(late + 1, CLOCK_STALE_REPLACE_RTT_MS, 1000)), true, 'a round trip of 1.5 s is reasonable and replaces the stale sample')
eq(o.offset, 1000, 'the new offset')
// the slow-reply scenario of the review: a good sample, six quiet minutes, then one 24-second round trip
const slow = new ServerClock(() => 0)
slow.sync(100_050, 100_000, 100_100) // offset 0
eq(slow.sync(...reply(100_000 + 6 * 60_000, 24_000, 0)), false, 'one very slow reply never shifts the deadline')
eq(slow.offset, 0, 'the offset is still the good one')
// a stale sample is replaced by a comparably good one (twice the kept round trip counts as reasonable above 1.5 s)
const wide = new ServerClock(() => 0)
wide.sync(1000, 0, 2000) // rtt 2000, offset 0
eq(wide.sync(...reply(CLOCK_SAMPLE_MAX_AGE_MS + 5000, 4000, 0)), true, 'a 4 s round trip against a kept 2 s one is within twice')
const wide2 = new ServerClock(() => 0)
wide2.sync(1000, 0, 2000)
eq(wide2.sync(...reply(CLOCK_SAMPLE_MAX_AGE_MS + 5000, 4001, 0)), false, 'but not 4.001 s')
// a device clock that went backwards
const b = new ServerClock(() => 0)
b.sync(100_000, 100_000, 100_010)
eq(b.sync(50_000, 50_000, 50_900), true, 'the clock went backwards: the new sample wins')
// the person changed the device clock: a clear jump with a short round trip is trusted
const j = new ServerClock(() => 0)
j.sync(100_000, 100_000, 100_010)
eq(j.sync(160_000 + CLOCK_JUMP_MS + 500, 130_000, 130_500), true, 'a jump of the offset with a short round trip is adopted')
const j2 = new ServerClock(() => 0)
j2.sync(100_000, 100_000, 100_010)
eq(j2.sync(100_000 + 20_000, 110_000, 114_000), false, 'a jump seen only through a slow round trip is not trusted')
j2.reset()
eq(j2.synced, false, 'reset forgets the measurement')
eq(j2.offset, 0, 'and the offset')

// --- display helpers --------------------------------------------------------------------------------------------------------------
eq(secondsLeft(1000, 5000), 0, 'never negative')
eq(secondsLeft(10_999, 0), 10, 'whole seconds, rounded down')
eq(clockText(0), '00:00', 'zero')
eq(clockText(65), '01:05', 'a minute and five seconds')
eq(clockText(90 * 60), '90:00', 'minutes keep counting past 59')
eq(clockText(-3), '00:00', 'never negative')
console.log('server clock tests ok')
