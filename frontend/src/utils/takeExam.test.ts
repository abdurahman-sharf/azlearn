import {
  CLOCK_STEP_TOLERANCE_MS, KEEPALIVE_MAX_BYTES, TIME_ANNOUNCE_MINUTES, announceDue, answeredCount, applySaved, changedAnswers, fitsKeepalive, isAnswered, isRetryableSubmitError,
  clockStepped, isTimedAttempt, radioMove, retryDelayMs, seedAnnounced, shouldAskBeforeLeaving,
} from './takeExam.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- partial autosave: only what changed since the last good save ---------------------------------------------------------------
eq(changedAnswers({ a: 'A', b: 'B' }, {}), { a: 'A', b: 'B' }, 'everything is new')
eq(changedAnswers({ a: 'A', b: 'B' }, { a: 'A', b: 'B' }), {}, 'nothing changed')
eq(changedAnswers({ a: 'A', b: 'C' }, { a: 'A', b: 'B' }), { b: 'C' }, 'only the changed question')
eq(changedAnswers({ a: '' }, { a: 'A' }), { a: '' }, 'an answer that was cleared is sent as empty')
eq(changedAnswers({}, { a: 'A' }), { a: '' }, 'a question that vanished from the page is cleared too')
eq(changedAnswers({ a: '   ' }, { a: 'A' }), { a: '' }, 'blank text counts as cleared')
eq(changedAnswers({ a: '' }, {}), {}, 'an empty box that was never saved is not a change')
eq(changedAnswers({ a: '' }, { a: '' }), {}, 'empty on both sides')
eq(changedAnswers({ a: ' x ' }, { a: 'x' }), { a: ' x ' }, 'text with different spaces is a change (the value is sent as typed)')
eq(applySaved({ a: 'A' }, { b: 'B' }), { a: 'A', b: 'B' }, 'merged')
eq(applySaved({ a: 'A', b: 'B' }, { a: '' }), { b: 'B' }, 'an empty value removes the key')
eq(applySaved({ a: 'A' }, { a: 'C' }), { a: 'C' }, 'overwritten')
// after a successful save the next diff is empty, and a later edit shows up alone
let saved: Record<string, string> = { a: 'A' }
const cur = { a: 'A', b: 'B' }
const sent = changedAnswers(cur, saved)
saved = applySaved(saved, sent)
eq(changedAnswers(cur, saved), {}, 'after saving: nothing left to send')
eq(changedAnswers({ ...cur, a: 'D' }, saved), { a: 'D' }, 'a later edit alone')
eq(isAnswered(' '), false, 'blank')
eq(isAnswered('A'), true, 'answered')
eq(isAnswered(undefined), false, 'undefined')
eq(answeredCount([{ id: 'a' }, { id: 'b' }, { id: 'c' }], { a: 'A', b: ' ', zz: 'x' }), 1, 'only the page questions count')
eq(fitsKeepalive('x'.repeat(100)), true, 'a small body fits')
eq(fitsKeepalive('x'.repeat(KEEPALIVE_MAX_BYTES + 1)), false, 'too big for keepalive')
eq(fitsKeepalive('ع'.repeat(KEEPALIVE_MAX_BYTES / 2 + 10)), false, 'bytes, not characters')

// --- back-off: 2, 4, 8, 16, then 30 seconds ------------------------------------------------------------------------------------
eq([1, 2, 3, 4, 5, 6, 50].map(retryDelayMs), [2000, 4000, 8000, 16000, 30000, 30000, 30000], 'exponential, capped at 30 s')
eq(retryDelayMs(0), 2000, 'never below the first pause')
eq(retryDelayMs(2.9), 4000, 'whole failures')
eq(isRetryableSubmitError({ code: 'network', status: 0 }), true, 'a dropped connection')
eq(isRetryableSubmitError({ code: 'unknown', status: 500 }), true, 'server error')
eq(isRetryableSubmitError({ code: 'unknown', status: 503 }), true, 'unavailable')
eq(isRetryableSubmitError({ code: 'rate_limited', status: 429 }), true, 'asked to slow down')
eq(isRetryableSubmitError({ code: 'time_expired', status: 409 }), false, 'the attempt is over: not an error to retry')
eq(isRetryableSubmitError({ code: 'already_submitted', status: 409 }), false, 'handed in elsewhere')
eq(isRetryableSubmitError({ code: 'forbidden', status: 403 }), false, 'a definite refusal')
eq(isRetryableSubmitError({ code: 'not_found', status: 404 }), false, 'gone')
eq(isRetryableSubmitError({ code: 'unauthorized', status: 401 }), false, 'a lost session goes to sign-in')
eq(isRetryableSubmitError(null), false, 'no error')

// --- announcements: 10, 5 and 1 minute, each once --------------------------------------------------------------------------------
eq([...TIME_ANNOUNCE_MINUTES], [10, 5, 1], 'the thresholds')
let ann = new Set<number>()
const step = (sec: number) => {
  const r = announceDue(sec, ann)
  ann = r.announced
  return r.minutes
}
eq(step(11 * 60), null, 'eleven minutes: nothing yet')
eq(step(10 * 60), 10, 'ten minutes left')
eq(step(10 * 60 - 1), null, 'only once')
eq(step(5 * 60 + 1), null, 'between')
eq(step(5 * 60), 5, 'five minutes left')
eq(step(60), 1, 'one minute left')
eq(step(59), null, 'once')
eq(step(0), null, 'nothing at zero')
// the page seeds what is already behind it when it first reads the time: nothing false is said on arrival
eq([...seedAnnounced(8 * 60)].sort((a, b) => a - b), [10], 'an 8-minute quiz: "10 minutes left" is behind it from the start')
eq([...seedAnnounced(3 * 60)].sort((a, b) => a - b), [5, 10], 'a 3-minute quiz starts past 10 and 5')
eq([...seedAnnounced(4 * 60)].sort((a, b) => a - b), [5, 10], 'a reload with four minutes left must not say "5 minutes left" again')
eq([...seedAnnounced(10 * 60)].sort((a, b) => a - b), [10], 'exactly ten minutes: that one is due now, i.e. spent')
eq([...seedAnnounced(10 * 60 + 1)], [], 'a long exam: nothing is behind it')
eq([...seedAnnounced(30)].sort((a, b) => a - b), [1, 5, 10], 'thirty seconds left on arrival: all spent, nothing said')
let seeded = seedAnnounced(8 * 60)
const next = (sec: number) => {
  const r = announceDue(sec, seeded)
  seeded = r.announced
  return r.minutes
}
eq(next(8 * 60), null, 'the first tick of an 8-minute quiz announces nothing')
eq(next(5 * 60), 5, 'five minutes left is still ahead and is said')
eq(next(60), 1, 'and one minute')
// extending the end re-seeds: the longer time gets its announcements
seeded = seedAnnounced(20 * 60)
eq(next(10 * 60), 10, 'after an extension to 20 minutes the 10-minute mark is said again')
// without seeding, a late arrival still names the nearest threshold (the function itself is unchanged)
const late = announceDue(4 * 60, new Set())
eq(late.minutes, 5, 'unseeded: four minutes left says five')
eq([...late.announced].sort((a, b) => a - b), [5, 10], 'and the larger one is spent')
eq(announceDue(30, new Set()).minutes, 1, 'thirty seconds unseeded says one minute')
eq(announceDue(-5, new Set()).minutes, null, 'negative')

// --- the device clock stepped between two ticks -------------------------------------------------------------------------------
eq(clockStepped(1000, 1000), false, 'a normal tick')
eq(clockStepped(1000 + CLOCK_STEP_TOLERANCE_MS, 1000), false, 'a little jitter is fine')
eq(clockStepped(1000 + CLOCK_STEP_TOLERANCE_MS + 1, 1000), true, 'a forward step (time correction, waking from sleep)')
eq(clockStepped(-3 * 60_000, 1000), true, 'a backward step')

// --- is there a countdown -----------------------------------------------------------------------------------------------------
const h = 3_600_000
eq(isTimedAttempt({ started_at: 0, ends_at: 60 * 60_000 }), true, 'a one-hour exam')
eq(isTimedAttempt({ started_at: 0, ends_at: 24 * h }), false, 'the 24-hour cap of an open-ended exam is no countdown')
eq(isTimedAttempt({ started_at: 0, ends_at: 24 * h, limited_by_closing: true }), true, 'a closing time that is exactly the cap still counts')
eq(isTimedAttempt({ started_at: 0, ends_at: 6 * h, limited_by_closing: true }), true, 'cut short by the closing time')
eq(isTimedAttempt({ started_at: 0, ends_at: 25 * h, limited_by_closing: false }), false, 'beyond the cap')

// --- leaving the page ---------------------------------------------------------------------------------------------------------
const base = { active: true, loggedIn: true, toPath: '/platform/student', fromPath: '/platform/assessments/x/take' }
eq(shouldAskBeforeLeaving(base), true, 'an open attempt and a move elsewhere')
eq(shouldAskBeforeLeaving({ ...base, active: false }), false, 'handed in or settled: free to go')
eq(shouldAskBeforeLeaving({ ...base, loggedIn: false }), false, 'a lost session reaches the sign-in page')
eq(shouldAskBeforeLeaving({ ...base, toPath: base.fromPath }), false, 'the same page')

// --- arrow keys in a radio group ------------------------------------------------------------------------------------------------
eq(radioMove('ArrowDown', 0, 4, false), 1, 'down')
eq(radioMove('ArrowDown', 3, 4, false), 0, 'down wraps')
eq(radioMove('ArrowUp', 0, 4, false), 3, 'up wraps')
eq(radioMove('ArrowRight', 1, 4, false), 2, 'right in LTR is next')
eq(radioMove('ArrowLeft', 1, 4, false), 0, 'left in LTR is previous')
eq(radioMove('ArrowRight', 1, 4, true), 0, 'right in RTL is previous')
eq(radioMove('ArrowLeft', 1, 4, true), 2, 'left in RTL is next')
eq(radioMove('Home', 2, 4, false), 0, 'home')
eq(radioMove('End', 0, 4, false), 3, 'end')
eq(radioMove('a', 0, 4, false), null, 'another key')
eq(radioMove('ArrowDown', 0, 0, false), null, 'no options')
console.log('take exam tests ok')
