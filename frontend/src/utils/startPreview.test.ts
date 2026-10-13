import { OPEN_ENDED_MS, startPreview } from './startPreview.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const NOW = 1_000_000_000
const MIN = 60_000

// a timed exam far from its closing time ends after its duration
eq(startPreview({ now: NOW, durationMin: 60, closesAt: NOW + 5 * 3_600_000, attemptsUsed: 0, maxAttempts: 2 }),
  { openEnded: false, durationMin: 60, endsAt: NOW + 60 * MIN, limitedByClosing: false, attemptNo: 1, attemptsMax: 2 }, 'duration first')
// started late: the closing time cuts the attempt short
eq(startPreview({ now: NOW, durationMin: 60, closesAt: NOW + 20 * MIN, attemptsUsed: 1, maxAttempts: 3 }),
  { openEnded: false, durationMin: 60, endsAt: NOW + 20 * MIN, limitedByClosing: true, attemptNo: 2, attemptsMax: 3 }, 'the hard end wins')
// both at the same moment: it is the closing time that ends it (the server caps there)
eq(startPreview({ now: NOW, durationMin: 30, closesAt: NOW + 30 * MIN, attemptsUsed: 0, maxAttempts: 1 }).limitedByClosing, true, 'a tie counts as the closing time')
// no duration, a closing time soon: the closing time ends it
eq(startPreview({ now: NOW, durationMin: null, closesAt: NOW + 2 * 3_600_000, attemptsUsed: 0, maxAttempts: 1 }),
  { openEnded: true, durationMin: null, endsAt: NOW + 2 * 3_600_000, limitedByClosing: true, attemptNo: 1, attemptsMax: 1 }, 'open-ended, closes soon')
// no duration, a closing time beyond the 24 h cap: nothing ends it within the cap
eq(startPreview({ now: NOW, durationMin: null, closesAt: NOW + OPEN_ENDED_MS + MIN, attemptsUsed: 0, maxAttempts: 1 }),
  { openEnded: true, durationMin: null, endsAt: null, limitedByClosing: false, attemptNo: 1, attemptsMax: 1 }, 'open-ended, closing far away')
// no duration, no closing time
eq(startPreview({ now: NOW, durationMin: undefined, closesAt: null, attemptsUsed: 0, maxAttempts: 1 }).endsAt, null, 'nothing ends it')
// a zero duration is no duration
eq(startPreview({ now: NOW, durationMin: 0, closesAt: null, attemptsUsed: 0, maxAttempts: 1 }).openEnded, true, 'zero minutes means open-ended')
// attempt counters never go below 1
eq(startPreview({ now: NOW, durationMin: 10, closesAt: null, attemptsUsed: -3, maxAttempts: 0 }).attemptNo, 1, 'attempt number floor')
eq(startPreview({ now: NOW, durationMin: 10, closesAt: null, attemptsUsed: -3, maxAttempts: 0 }).attemptsMax, 1, 'attempt limit floor')
console.log('start preview tests ok')
