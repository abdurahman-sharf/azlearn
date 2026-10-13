import { difficultyKey, formatDuration, hasAttemptMeta, releaseNotice } from './attemptMeta.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(formatDuration(0), '0:00', 'zero')
eq(formatDuration(65), '1:05', 'a minute and five seconds')
eq(formatDuration(59.9), '0:59', 'whole seconds')
eq(formatDuration(3600), '1:00:00', 'an hour')
eq(formatDuration(3725), '1:02:05', 'hours, minutes, seconds')
eq(formatDuration(null), null, 'unknown')
eq(formatDuration(undefined), null, 'absent')
eq(formatDuration(-1), null, 'negative')
eq(formatDuration(Number.NaN), null, 'NaN')
eq(hasAttemptMeta({}), false, 'an older server sends nothing')
eq(hasAttemptMeta({ attempt_no: 2 }), true, 'attempt number')
eq(hasAttemptMeta({ is_best: true }), true, 'best')
eq(hasAttemptMeta({ is_best: false }), false, 'not best alone shows nothing')
eq(hasAttemptMeta({ submitted_at: 5 }), true, 'submitted time')
eq(difficultyKey('easy'), 'bank_d_easy', 'known')
eq(difficultyKey('hard'), 'bank_d_hard', 'known')
eq(difficultyKey('weird'), null, 'unknown value')
eq(difficultyKey(null), null, 'none')
// --- the withheld-result notice: a release moment that already passed must not be shown as a time ---------------------------------
eq(releaseNotice(2_000, 1_000), 'at', 'the closing time is still ahead: show it')
eq(releaseNotice(1_000, 1_000), 'soon', 'reached: the result follows once the last save could have landed')
eq(releaseNotice(500, 1_000), 'soon', 'already past (inside the grace): "shortly", not a time in the past')
eq(releaseNotice(null, 1_000), 'none', 'no moment known')
eq(releaseNotice(undefined, 1_000), 'none', 'older server')
eq(releaseNotice(Number.NaN, 1_000), 'none', 'nonsense')
console.log('attempt meta tests ok')
