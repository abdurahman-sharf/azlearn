import {
  DEFAULT_STEX_FILTERS, OPEN_RECHECK_MAX, OPEN_RECHECK_MS, bestView, countdown, examsBadge, fmtScore, openRecheck, overviewEmpty, overviewSteps, pct, reasonKey,
  rowLinks, runningButUnreachable, showOverviewChecklist, stateChip, stexClampPage, stexFiltersFromQuery, stexFiltersToQuery, stexListQuery, tabCount,
} from './studentExams.ts'
import type { StudentExamRow, StudentOverview } from '@/api/platformStudent'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- the address bar and the request ------------------------------------------------------------------------------------
eq(stexFiltersToQuery(DEFAULT_STEX_FILTERS), {}, 'defaults leave the address clean')
eq(stexFiltersToQuery({ tab: 'done', q: ' math ', subjectId: 's1', page: 2 }), { q: 'math', tab: 'done', subject: 's1', page: '3' }, 'only what differs, one-based page')
eq(stexFiltersFromQuery({ tab: 'in_progress', q: 'x', subject: 's2', page: '2' }), { tab: 'in_progress', q: 'x', subjectId: 's2', page: 1 }, 'read back')
eq(stexFiltersFromQuery({ tab: 'draft', page: '0' }), DEFAULT_STEX_FILTERS, 'a teacher tab or page 0 is not ours: defaults')
eq(stexFiltersFromQuery({ tab: ['missed', 'open'], page: ['-1'] }), { ...DEFAULT_STEX_FILTERS, tab: 'missed' }, 'repeated keys use the first, negative page ignored')
eq(stexFiltersFromQuery({ q: 'a'.repeat(300) }).q.length, 100, 'search text is capped')
eq(stexFiltersFromQuery(stexFiltersToQuery({ tab: 'upcoming', q: 'q', subjectId: 'z', page: 5 })), { tab: 'upcoming', q: 'q', subjectId: 'z', page: 5 }, 'round trip')
eq(stexListQuery(DEFAULT_STEX_FILTERS), { limit: 25, offset: 0 }, 'all: no tab sent')
eq(stexListQuery({ tab: 'open', q: '  q ', subjectId: 's', page: 1 }), { limit: 25, offset: 25, tab: 'open', q: 'q', subject_id: 's' }, 'tab, search, subject, page')
eq(stexListQuery({ ...DEFAULT_STEX_FILTERS, page: -2 }).offset, 0, 'negative page is the first')

// --- tab counts and paging ------------------------------------------------------------------------------------------------
const counts = { open: 2, upcoming: 1, in_progress: 1, done: 4, missed: 3 }
eq(tabCount('all', counts), 11, 'all is the sum')
eq(tabCount('done', counts), 4, 'one state')
eq(tabCount('missed', null), 0, 'unknown counts')
eq(tabCount('all', { open: 1 }), 1, 'missing states count as zero')
eq(stexClampPage(4, 51), 2, 'the last page that exists')
eq(stexClampPage(0, 0), 0, 'empty list')

// --- row links ------------------------------------------------------------------------------------------------------------
const row = (p: Partial<StudentExamRow>): StudentExamRow => ({
  id: 'e1', title: 'T', subject_id: 's', subject_name: 'S', teacher_name: 'Teacher', question_count: 3, total_points: 10, duration_min: 30,
  opens_at: null, closes_at: null, max_attempts: 1, pass_mark: null, release_mode: 'immediate', phase: 'published', state: 'open',
  can_start: true, reason: null, attempts_used: 0, in_progress_attempt: null, best: null, last_attempt_id: null, visible: true, ...p,
})
eq(rowLinks(row({})), { start: '/platform/assessments/e1', resume: null, result: null }, 'open: start goes through the exam page')
eq(rowLinks(row({ state: 'in_progress', can_start: false, in_progress_attempt: { attempt_id: 'a1', ends_at: 1 }, last_attempt_id: 'a1' })),
  { start: null, resume: '/platform/assessments/e1/take', result: null }, 'in progress: resume only')
const best = { attempt_id: 'a2', score: 7, total: 10, percent: 70, passed: true, released: true }
eq(rowLinks(row({ state: 'done', can_start: false, reason: 'no_attempts', best, last_attempt_id: 'a3' })), { start: null, resume: null, result: '/platform/attempts/a2' }, 'done: the best attempt')
eq(rowLinks(row({ state: 'open', attempts_used: 1, max_attempts: 3, best, last_attempt_id: 'a2' })).result, '/platform/attempts/a2', 'open again with a result: both start and result')
eq(rowLinks(row({ state: 'missed', can_start: false, reason: 'closed', last_attempt_id: 'x9' })).result, '/platform/attempts/x9', 'missed with an expired attempt: it is still shown')
eq(rowLinks(row({ state: 'missed', can_start: false, reason: 'closed' })).result, null, 'missed without any attempt: nothing to open')
eq(rowLinks(row({ visible: false, can_start: true })).start, null, 'an exam no longer shown can never be started from the list')
const runningRow = { state: 'in_progress' as const, can_start: false, in_progress_attempt: { attempt_id: 'a1', ends_at: 1 }, last_attempt_id: 'a1' }
eq(rowLinks(row({ ...runningRow, visible: false })), { start: null, resume: null, result: null }, 'a running attempt on an exam I can no longer open: no resume (the server would answer 404 / 403)')
eq(rowLinks(row({ ...runningRow, visible: false, best })).resume, null, 'hidden and running with an earlier result: still no resume')
eq(rowLinks(row({ ...runningRow, visible: false, best })).result, '/platform/attempts/a2', 'but the earlier result stays reachable')
eq(runningButUnreachable(row({ ...runningRow, visible: false })), true, 'running and unreachable')
eq(runningButUnreachable(row({ ...runningRow, visible: true })), false, 'running and reachable')
eq(runningButUnreachable(row({ visible: false })), false, 'not running')
// the server says whether the running attempt can be re-entered: an enrolled student gets back into a sitting whose exam was hidden meanwhile
eq(rowLinks(row({ ...runningRow, visible: false, can_resume: true })), { start: null, resume: '/platform/assessments/e1/take', result: null }, 'hidden meanwhile but still enrolled: resume')
eq(runningButUnreachable(row({ ...runningRow, visible: false, can_resume: true })), false, 'so it is not "unreachable"')
eq(rowLinks(row({ ...runningRow, visible: true, can_resume: false })).resume, null, 'not enrolled any more: no resume (the server answers 404 / 403)')
eq(runningButUnreachable(row({ ...runningRow, visible: false, can_resume: false })), true, 'running and unreachable')

// --- the state chip -------------------------------------------------------------------------------------------------------
eq(stateChip('missed', 'not_enrolled'), 'not_enrolled', 'open but not enrolled is never "missed"')
eq(stateChip('missed', 'closed'), 'missed', 'really missed')
eq(stateChip('missed', 'no_attempts'), 'missed', 'attempts used up')
eq(stateChip('upcoming', 'not_enrolled'), 'upcoming', 'upcoming stays upcoming')
eq(stateChip('done', 'not_enrolled'), 'done', 'done stays done')
eq(stateChip('open', null), 'open', 'open')
eq(stateChip(null, null), null, 'no state (staff)')
eq(rowLinks(row({ state: 'upcoming', can_start: false, reason: 'not_open' })), { start: null, resume: null, result: null }, 'upcoming: nothing yet')

// --- best result ----------------------------------------------------------------------------------------------------------
eq(bestView(null), { kind: 'none' }, 'no attempt')
eq(bestView({ ...best, released: false, score: null, percent: null, passed: null }), { kind: 'withheld' }, 'withheld until the exam closes')
eq(bestView({ ...best, released: true, score: null }), { kind: 'withheld' }, 'released without a score is treated as withheld')
eq(bestView(best), { kind: 'score', score: 7, total: 10, percent: 70, passed: true }, 'released score')
eq(fmtScore(7), '7', 'integer')
eq(fmtScore(7.25), '7.3', 'one decimal')
eq(fmtScore(6.999), '7', 'no trailing .0')

// --- countdown ------------------------------------------------------------------------------------------------------------
eq(countdown(1000 + (2 * 86_400 + 3 * 3600 + 4 * 60 + 5) * 1000, 1000), { days: 2, hours: 3, minutes: 4, seconds: 5, done: false }, 'd h m s')
eq(countdown(500, 1000), { days: 0, hours: 0, minutes: 0, seconds: 0, done: true }, 'past: zero, done')
eq(countdown(1999, 1000).done, false, 'under a second left is NOT reached yet (a request sent now would still be answered "not open")')
eq(countdown(1999, 1000).seconds, 0, 'but it shows zero')
eq(countdown(2000, 2000).done, true, 'the instant itself is reached')
eq(countdown(2000, 1000).seconds, 1, 'one second')

// --- asking again once the opening time has passed on this device ---------------------------------------------------------
const rc = (p: Partial<{ state: string | null; opensAt: number | null; now: number; tries: number; lastTryAt: number }>) =>
  openRecheck({ state: 'upcoming', opensAt: 10_000, now: 10_000, tries: 0, lastTryAt: 0, ...p })
eq(rc({ now: 9_999 }), 'wait', 'before the opening time: the countdown runs')
eq(rc({}), 'retry', 'at the opening time: ask the server')
eq(rc({ tries: 1, lastTryAt: 10_000, now: 10_000 + OPEN_RECHECK_MS - 1 }), 'wait', 'asked a moment ago')
eq(rc({ tries: 1, lastTryAt: 10_000, now: 10_000 + OPEN_RECHECK_MS }), 'retry', 'still upcoming after a few seconds: ask again (a clock that runs ahead must not strand the page)')
eq(rc({ tries: OPEN_RECHECK_MAX - 1, lastTryAt: 0, now: 99_999 }), 'retry', 'the last allowed ask')
eq(rc({ tries: OPEN_RECHECK_MAX, lastTryAt: 0, now: 99_999 }), 'give_up', 'enough asks: hand it to the student')
eq(rc({ state: 'open' }), 'wait', 'the server says open: nothing more to do')
eq(rc({ state: null }), 'wait', 'not a student view')
eq(rc({ opensAt: null }), 'wait', 'no opening time')

eq(reasonKey('not_enrolled'), 'stexReason_not_enrolled', 'reason key')
eq(reasonKey(null), null, 'no reason')

// --- overview -------------------------------------------------------------------------------------------------------------
const ov = (p: Partial<StudentOverview>): StudentOverview => ({
  enrolled: 2, unread: 0, resume: null, exams: { open: 0, upcoming: 0, in_progress: 0, closing_soon: [] }, awaiting_grading: 0,
  recent_results: [], courses: [], next_live: null, checklist: { placement: true, enrolled: true }, ...p,
})
eq(overviewSteps(null), [], 'nothing loaded: no steps')
eq(overviewSteps(ov({ checklist: { placement: false, enrolled: true } })), [{ id: 'placement', done: false }, { id: 'enrol', done: true }], 'steps')
eq(showOverviewChecklist(overviewSteps(ov({}))), false, 'both done: checklist hidden')
eq(showOverviewChecklist(overviewSteps(ov({ checklist: { placement: true, enrolled: false } }))), true, 'not enrolled yet: shown')
eq(showOverviewChecklist(overviewSteps(ov({ checklist: { placement: false, enrolled: false } }))), true, 'nothing done: shown')
eq(showOverviewChecklist(overviewSteps(ov({ checklist: { placement: false, enrolled: true } }))), false, 'enrolled without ever choosing a level: the checklist can be cleared')
eq(showOverviewChecklist([]), false, 'no steps')
eq(examsBadge(ov({ exams: { open: 2, upcoming: 5, in_progress: 1, closing_soon: [] } })), 3, 'badge = open + in progress')
eq(examsBadge(null), 0, 'no numbers yet')
eq(overviewEmpty(ov({})), true, 'all quiet: the empty state')
eq(overviewEmpty(ov({ checklist: { placement: false, enrolled: true } })), true, 'enrolled without a level and nothing waiting: the empty state too')
eq(overviewEmpty(ov({ checklist: { placement: false, enrolled: false } })), false, 'the checklist is something to do')
eq(overviewEmpty(ov({ exams: { open: 0, upcoming: 1, in_progress: 0, closing_soon: [] } })), false, 'an upcoming exam')
eq(overviewEmpty(ov({ awaiting_grading: 1 })), false, 'awaiting grading')
eq(overviewEmpty(ov({ next_live: { id: 'l', title: 'L', subject_id: 's', subject_name: 'S', starts_at: 1, duration_min: 30 } })), false, 'a live lesson')
eq(overviewEmpty(null), false, 'not loaded is not empty')
eq(pct(45.6), 46, 'rounded')
eq(pct(140), 100, 'clamped high')
eq(pct(undefined), 0, 'missing')
console.log('student exams tests ok')
