import { AUTO_SUBMIT_BACKOFF_MS, attemptDraftKey, attemptDraftKeys, autoSubmitDue, isAttemptOverCode, removeAttemptDrafts } from './attemptDrafts.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

/** A tiny in-memory Storage whose `key(i)` shifts when an item is removed, like the real one. */
function mem(init: Record<string, string>) {
  const data = new Map(Object.entries(init))
  return {
    get length() { return data.size },
    key: (i: number) => [...data.keys()][i] ?? null,
    removeItem: (k: string) => { data.delete(k) },
    keys: () => [...data.keys()],
  }
}

eq(attemptDraftKey('abc'), 'exameow-attempt-abc', 'the key the exam page writes')

const s = mem({
  'exameow-attempt-a1': '{}', 'exameow-banks': '[]', 'exameow-attempt-a2': '{"q":"A"}', 'exameow-platform-token': 't', 'exameow-attempt-a3': '{}',
})
eq(attemptDraftKeys(s), ['exameow-attempt-a1', 'exameow-attempt-a2', 'exameow-attempt-a3'], 'finds every draft and nothing else')
eq(removeAttemptDrafts(s), 3, 'removes all three although the indexes shift')
eq(s.keys(), ['exameow-banks', 'exameow-platform-token'], 'banks and the session token are left alone')
eq(removeAttemptDrafts(s), 0, 'nothing left')
eq(attemptDraftKeys(null), [], 'no storage')
eq(removeAttemptDrafts(undefined), 0, 'no storage to clean')
const broken = { get length(): number { throw new Error('blocked') }, key: () => null, removeItem: () => { /* */ } }
eq(attemptDraftKeys(broken), [], 'blocked storage never throws')
// --- the attempt is over on the server: leave the page, never loop -----------------------------------------------------------
eq(isAttemptOverCode('time_expired'), true, 'settled when time ran out (the device slept through the deadline)')
eq(isAttemptOverCode('already_submitted'), true, 'submitted from another tab or device')
eq(isAttemptOverCode('network'), false, 'a lost connection is not the end of the attempt')
eq(isAttemptOverCode('not_found'), false, 'another error')
eq(isAttemptOverCode(undefined), false, 'no code')
const due = (p: Partial<{ timed: boolean; remainingSec: number; submitting: boolean; over: boolean; now: number; lastTryAt: number }>) =>
  autoSubmitDue({ timed: true, remainingSec: 0, submitting: false, over: false, now: 100_000, lastTryAt: 0, ...p })
eq(due({}), true, 'time is up and nothing was tried')
eq(due({ remainingSec: 1 }), false, 'time is left')
eq(due({ timed: false }), false, 'an untimed attempt never auto-submits')
eq(due({ submitting: true }), false, 'already sending')
eq(due({ over: true }), false, 'the server said it is over: no more requests')
eq(due({ now: 100_000, lastTryAt: 100_000 - AUTO_SUBMIT_BACKOFF_MS + 1 }), false, 'the last try failed a moment ago: wait')
eq(due({ now: 100_000, lastTryAt: 100_000 - AUTO_SUBMIT_BACKOFF_MS }), true, 'after the pause: try again')
console.log('attempt drafts tests ok')
