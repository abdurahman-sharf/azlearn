import type {
  BestAttempt, StudentExamCounts, StudentExamQuery, StudentExamReason, StudentExamRow, StudentExamState, StudentExamTab, StudentOverview,
} from '@/api/platformStudent'

// Pure rules of the student's screens: the "My exams" tabs and the filters kept in the address bar, what a row offers,
// how a best result is shown, the countdown to an exam that has not opened yet, and the overview's checklist. Kept out of
// the views so a plain script can test them. The state itself is always the server's (one rule for every screen).

export const STUDENT_TABS: StudentExamTab[] = ['all', 'open', 'upcoming', 'in_progress', 'done', 'missed']
export const STUDENT_STATES: StudentExamState[] = ['in_progress', 'open', 'upcoming', 'done', 'missed']
export const STEX_PER = 25

export interface StudentExamFilters {
  tab: StudentExamTab
  q: string
  subjectId: string
  /** zero-based */
  page: number
}
export const DEFAULT_STEX_FILTERS: StudentExamFilters = { tab: 'all', q: '', subjectId: '', page: 0 }

const first = (v: unknown): string => (Array.isArray(v) ? String(v[0] ?? '') : typeof v === 'string' ? v : '')

/** Reads the filters back from a route query; anything unknown or malformed falls back to the default. */
export function stexFiltersFromQuery(query: Record<string, unknown>): StudentExamFilters {
  const tab = first(query.tab) as StudentExamTab
  const page = Number.parseInt(first(query.page), 10)
  return {
    tab: STUDENT_TABS.includes(tab) ? tab : 'all',
    q: first(query.q).slice(0, 100),
    subjectId: first(query.subject).slice(0, 64),
    page: Number.isFinite(page) && page > 0 ? Math.min(page - 1, 10_000) : 0,
  }
}

/** The route query for the filters: only what differs from the default, the page one-based. */
export function stexFiltersToQuery(f: StudentExamFilters): Record<string, string> {
  const out: Record<string, string> = {}
  if (f.q.trim()) out.q = f.q.trim()
  if (f.tab !== 'all') out.tab = f.tab
  if (f.subjectId) out.subject = f.subjectId
  if (f.page > 0) out.page = String(f.page + 1)
  return out
}

/** The list request for the filters (`all` sends no tab). */
export function stexListQuery(f: StudentExamFilters, per = STEX_PER): StudentExamQuery {
  const out: StudentExamQuery = { limit: per, offset: Math.max(0, f.page) * per }
  if (f.tab !== 'all') out.tab = f.tab
  if (f.q.trim()) out.q = f.q.trim()
  if (f.subjectId) out.subject_id = f.subjectId
  return out
}

/** The number shown on a tab: `all` is the sum of the states (the server counts over the whole scope, ignoring the tab). */
export function tabCount(tab: StudentExamTab, counts: Partial<StudentExamCounts> | null | undefined): number {
  if (!counts) return 0
  if (tab === 'all') return STUDENT_STATES.reduce((n, s) => n + (counts[s] ?? 0), 0)
  return counts[tab] ?? 0
}

/** The last page that exists for `total` rows. */
export function stexClampPage(page: number, total: number, per = STEX_PER): number {
  return Math.min(Math.max(0, page), Math.max(1, Math.ceil(Math.max(0, total) / per)) - 1)
}

export interface StudentRowLinks {
  /** the exam page (rules and the start button); only when I can start it now */
  start: string | null
  /** back into the running attempt */
  resume: string | null
  /** my best submitted attempt, else my last finished one */
  result: string | null
}

type LinkRow = Pick<StudentExamRow, 'id' | 'state' | 'can_start' | 'visible' | 'can_resume' | 'in_progress_attempt' | 'best' | 'last_attempt_id'>

/** The server says whether the running attempt can be re-entered (`can_resume`); an older server sends no such field and `visible` decides. */
const resumable = (r: Pick<StudentExamRow, 'visible' | 'can_resume'>): boolean => r.can_resume ?? r.visible

/**
 * Where a row of "My exams" can lead. Starting goes through the exam page (its rules, and from phase 4-3 a confirmation),
 * never straight into an attempt. A row that is no longer visible (unenrolled, hidden, archived) never offers a start and only
 * leads to my own results - except that a sitting in progress can be resumed while the student is still enrolled, even when the
 * exam was hidden meanwhile (`can_resume`); once they are not enrolled the server refuses (404 / 403) and no resume is offered.
 */
export function rowLinks(r: LinkRow): StudentRowLinks {
  const running = r.state === 'in_progress' && !!r.in_progress_attempt
  const result = r.best
    ? `/platform/attempts/${r.best.attempt_id}`
    : !running && r.last_attempt_id && (r.state === 'done' || r.state === 'missed')
      ? `/platform/attempts/${r.last_attempt_id}`
      : null
  return {
    start: r.can_start && r.visible && !running ? `/platform/assessments/${r.id}` : null,
    resume: running && resumable(r) ? `/platform/assessments/${r.id}/take` : null,
    result,
  }
}

/** An attempt is still running but the student can no longer get back into it (not enrolled any more): it ends by itself at its deadline. */
export const runningButUnreachable = (r: Pick<StudentExamRow, 'state' | 'visible' | 'can_resume' | 'in_progress_attempt'>): boolean =>
  r.state === 'in_progress' && !resumable(r) && !!r.in_progress_attempt

export type StateChip = StudentExamState | 'not_enrolled'

/**
 * The chip shown for an exam. The server files "open, but you are not enrolled" under `missed` (its `can_start` is exactly
 * `state == open`) with the reason `not_enrolled`; telling a student who can still take the exam that they "missed" it
 * would be wrong, so that combination reads "Not enrolled" (the reason text and the enrol button say the rest).
 */
export function stateChip(state: StudentExamState | null | undefined, reason: StudentExamReason | null | undefined): StateChip | null {
  if (!state) return null
  return state === 'missed' && reason === 'not_enrolled' ? 'not_enrolled' : state
}

export type BestView =
  | { kind: 'none' }
  /** submitted, but the result is withheld until the exam closes */
  | { kind: 'withheld' }
  | { kind: 'score'; score: number; total: number; percent: number | null; passed: boolean | null }

export function bestView(b: BestAttempt | null | undefined): BestView {
  if (!b) return { kind: 'none' }
  if (!b.released || b.score === null) return { kind: 'withheld' }
  return { kind: 'score', score: b.score, total: b.total, percent: b.percent, passed: b.passed }
}

/** A score for display: at most one decimal, no trailing ".0". */
export function fmtScore(n: number): string {
  return String(Math.round(n * 10) / 10)
}

export interface Countdown { days: number; hours: number; minutes: number; seconds: number; done: boolean }

/**
 * Time left until `target` (epoch ms) from `now`, never negative. `done` only once the instant itself is reached (not when
 * less than a second is left): the page asks the server at that moment, and a request sent before the opening instant is
 * answered "not open yet".
 */
export function countdown(target: number, now: number): Countdown {
  const left = Math.max(0, Math.floor((target - now) / 1000))
  return {
    days: Math.floor(left / 86_400),
    hours: Math.floor((left % 86_400) / 3600),
    minutes: Math.floor((left % 3600) / 60),
    seconds: left % 60,
    done: target <= now,
  }
}

/** How long the exam page waits between two asks, and how many asks it makes before it hands the decision to the student. */
export const OPEN_RECHECK_MS = 3000
export const OPEN_RECHECK_MAX = 10

export type OpenRecheck = 'wait' | 'retry' | 'give_up'

/**
 * What the exam page does about an exam that is still `upcoming` on the server's answer. Once this device's clock reaches
 * `opens_at` the page asks the server again - and keeps asking every [`OPEN_RECHECK_MS`] while the answer is still
 * "upcoming" (the server's clock may be a little behind this device's, or the first answer was sent a few milliseconds
 * before the opening instant), up to [`OPEN_RECHECK_MAX`] times; then it stops and shows a button. A single reload at the
 * opening instant would leave the student on "upcoming" with no timer and no start button.
 */
export function openRecheck(i: { state: string | null | undefined; opensAt: number | null | undefined; now: number; tries: number; lastTryAt: number }): OpenRecheck {
  if (i.state !== 'upcoming' || !i.opensAt || i.now < i.opensAt) return 'wait'
  if (i.tries >= OPEN_RECHECK_MAX) return 'give_up'
  return i.tries === 0 || i.now - i.lastTryAt >= OPEN_RECHECK_MS ? 'retry' : 'wait'
}

/** The reason text key for a row/exam that cannot be started (null when there is nothing to explain). */
export function reasonKey(reason: StudentExamReason | null | undefined): `stexReason_${StudentExamReason}` | null {
  return reason ? `stexReason_${reason}` : null
}

// ── overview ─────────────────────────────────────────────────────────────────────────────────────────────────────────

export type OverviewStep = 'placement' | 'enrol'
export interface StepState { id: OverviewStep; done: boolean }

/** The first-run checklist: choose my level, then enrol in subjects. */
export function overviewSteps(o: Pick<StudentOverview, 'checklist'> | null | undefined): StepState[] {
  if (!o) return []
  return [
    { id: 'placement', done: !!o.checklist?.placement },
    { id: 'enrol', done: !!o.checklist?.enrolled },
  ]
}
/**
 * Shown until the student is enrolled in a subject. Choosing a level is a shortcut to suggestions, not a duty: a student who
 * enrolled straight from a subject page (so never chose a level) would otherwise see "Start here" on every visit and never
 * the empty state.
 */
export const showOverviewChecklist = (steps: StepState[]) => steps.some((s) => s.id === 'enrol' && !s.done)

/** The sidebar's "My exams" badge: what I can act on now (start or resume). */
export function examsBadge(o: Pick<StudentOverview, 'exams'> | null | undefined): number {
  return o ? (o.exams?.open ?? 0) + (o.exams?.in_progress ?? 0) : 0
}

/** Nothing to show on the overview (the checklist is done and every section is empty): the page says so and points to my subjects. */
export function overviewEmpty(o: StudentOverview | null | undefined): boolean {
  if (!o) return false
  return (
    !o.resume &&
    !(o.exams?.closing_soon?.length) &&
    (o.exams?.open ?? 0) === 0 &&
    (o.exams?.upcoming ?? 0) === 0 &&
    (o.awaiting_grading ?? 0) === 0 &&
    !(o.recent_results?.length) &&
    !(o.courses?.length) &&
    !o.next_live &&
    !showOverviewChecklist(overviewSteps(o))
  )
}

/** A course's progress for display, clamped to 0..100 and rounded. */
export const pct = (n: number | null | undefined) => Math.min(100, Math.max(0, Math.round(n ?? 0)))
