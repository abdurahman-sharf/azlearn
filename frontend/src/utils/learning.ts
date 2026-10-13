import type { LearningCounts, LearningItem, LearningQuery, LearningState, LearningTab } from '@/api/platformStudent'

// Pure rules of "My learning": the state tabs and the filters kept in the address bar (same pattern as "My exams"), the
// tab counts, paging, and where a row leads. Kept out of the view so a plain script can test them. The state of a course
// (not started / in progress / completed) is always the server's.

export const LEARNING_TABS: LearningTab[] = ['all', 'in_progress', 'not_started', 'completed']
export const LEARNING_STATES: LearningState[] = ['in_progress', 'not_started', 'completed']
export const LEARNING_PER = 24

export interface LearningFilters {
  tab: LearningTab
  q: string
  subjectId: string
  /** zero-based */
  page: number
}
export const DEFAULT_LEARNING_FILTERS: LearningFilters = { tab: 'all', q: '', subjectId: '', page: 0 }

const first = (v: unknown): string => (Array.isArray(v) ? String(v[0] ?? '') : typeof v === 'string' ? v : '')

/** Reads the filters back from a route query; anything unknown or malformed falls back to the default. */
export function learningFiltersFromQuery(query: Record<string, unknown>): LearningFilters {
  const tab = first(query.tab) as LearningTab
  const page = Number.parseInt(first(query.page), 10)
  return {
    tab: LEARNING_TABS.includes(tab) ? tab : 'all',
    q: first(query.q).slice(0, 100),
    subjectId: first(query.subject).slice(0, 64),
    page: Number.isFinite(page) && page > 0 ? Math.min(page - 1, 10_000) : 0,
  }
}

/** The route query for the filters: only what differs from the default, the page one-based. */
export function learningFiltersToQuery(f: LearningFilters): Record<string, string> {
  const out: Record<string, string> = {}
  if (f.q.trim()) out.q = f.q.trim()
  if (f.tab !== 'all') out.tab = f.tab
  if (f.subjectId) out.subject = f.subjectId
  if (f.page > 0) out.page = String(f.page + 1)
  return out
}

/** The list request for the filters (`all` sends no state). */
export function learningListQuery(f: LearningFilters, per = LEARNING_PER): LearningQuery {
  const out: LearningQuery = { limit: per, offset: Math.max(0, f.page) * per }
  if (f.tab !== 'all') out.state = f.tab
  if (f.q.trim()) out.q = f.q.trim()
  if (f.subjectId) out.subject_id = f.subjectId
  return out
}

/** The number shown on a tab: `all` is the sum of the states (the server counts over the filtered scope, ignoring the tab). */
export function learningTabCount(tab: LearningTab, counts: Partial<LearningCounts> | null | undefined): number {
  if (!counts) return 0
  if (tab === 'all') return LEARNING_STATES.reduce((n, s) => n + (counts[s] ?? 0), 0)
  return counts[tab] ?? 0
}

/** The last page that exists for `total` rows. */
export function learningClampPage(page: number, total: number, per = LEARNING_PER): number {
  return Math.min(Math.max(0, page), Math.max(1, Math.ceil(Math.max(0, total) / per)) - 1)
}

type LinkItem = Pick<LearningItem, 'course_id' | 'next_lesson_id'>

/** "Continue": the course page opened on the next lesson I have not completed (the course itself when there is none). */
export function continueLink(i: LinkItem): string {
  const base = `/platform/courses/${i.course_id}`
  return i.next_lesson_id ? `${base}?lesson=${encodeURIComponent(i.next_lesson_id)}` : base
}

/** A row offers "Review" (the course from its first lesson) once it is completed, "Continue" (or "Start") otherwise. */
export const rowAction = (i: Pick<LearningItem, 'state'>): 'review' | 'continue' | 'start' =>
  i.state === 'completed' ? 'review' : i.state === 'not_started' ? 'start' : 'continue'

/** Progress for display: whole-number percent clamped to 0..100, and the "n / m" pair. */
export function progressView(i: Pick<LearningItem, 'completed' | 'total' | 'percent'>): { percent: number; label: string } {
  const percent = Math.min(100, Math.max(0, Math.round(Number.isFinite(i.percent) ? i.percent : 0)))
  return { percent, label: `${i.completed} / ${i.total}` }
}
