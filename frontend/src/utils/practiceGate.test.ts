import { PRACTICE_GRACE_MS, practiceAccess, resultsReleased, type PracticeFacts } from './practiceGate.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const base: PracticeFacts = { allowPractice: true, reason: null, state: 'open', status: 'published', opensAt: null, releaseMode: 'immediate', closesAt: null, now: 1_000_000 }
const a = (p: Partial<PracticeFacts>) => practiceAccess({ ...base, ...p })

eq(a({}), 'available', 'on, enrolled, open, results shown at once')
eq(a({ allowPractice: false }), 'hidden', 'the owner has not allowed it')
eq(a({ allowPractice: undefined }), 'hidden', 'an older server sends no flag')
eq(a({ reason: 'not_enrolled' }), 'hidden', 'outside the subject')
eq(a({ state: 'in_progress' }), 'hidden', 'a graded attempt is running: the key is not for the sitting it is in')
eq(a({ state: 'done' }), 'available', 'finished attempts: revise')
eq(a({ state: 'missed', status: 'closed' }), 'available', 'a closed exam can be practised')
eq(a({ reason: 'no_attempts', state: 'done' }), 'available', 'attempts used up: practice is still there')
// before the exam opens
eq(a({ opensAt: 1_000_001 }), 'later', 'not open yet')
eq(a({ opensAt: 1_000_000 }), 'available', 'the opening time itself')
eq(a({ opensAt: 2_000_000, status: 'closed' }), 'available', 'the opening time only matters while the exam is published')
// the results embargo
const embargo = { releaseMode: 'after_close', closesAt: 2_000_000 }
eq(a({ ...embargo }), 'later', 'withheld until the exam closes')
eq(a({ ...embargo, now: 2_000_000 + PRACTICE_GRACE_MS }), 'later', 'the last millisecond a save can still land')
eq(a({ ...embargo, now: 2_000_000 + PRACTICE_GRACE_MS + 1 }), 'available', 'released')
eq(a({ ...embargo, status: 'closed' }), 'available', 'closed by hand: everything was ended at the close')
eq(a({ releaseMode: 'after_close', closesAt: null }), 'later', 'no closing time: nothing releases it by time')
eq(resultsReleased({ releaseMode: 'immediate', status: 'published', closesAt: null, now: 0 }), true, 'immediate is always released')
eq(resultsReleased({ releaseMode: 'after_close', status: 'archived', closesAt: null, now: 0 }), true, 'archived is released')
console.log('practice gate tests ok')
