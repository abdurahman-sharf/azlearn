import type { StatsTrendPoint, SubjectStats, StudentStats } from '@/api/platformStudent'

// The "My progress" page: the student's own results by subject. Everything private to the student, no ranking and nobody
// else's figures. The trend is drawn with plain CSS bars (no chart library); these helpers decide the bar sizes and the words.

/** Below this a chapter is listed as weak (the server applies the same line). */
export const WEAK_BELOW_PERCENT = 70

export const clampPercent = (p: number | null | undefined): number => {
  const n = typeof p === 'number' && Number.isFinite(p) ? p : 0
  return Math.min(100, Math.max(0, n))
}

/** A percentage for display: at most one decimal, no trailing ".0" ("85", "72.5"). `null` when there is none. */
export function fmtPercent(p: number | null | undefined): string | null {
  if (typeof p !== 'number' || !Number.isFinite(p)) return null
  return String(Math.round(p * 10) / 10)
}

/** Height of a trend bar as a share of the chart's height, 0-100. A 0 % result still shows as a thin line (see `minBar`). */
export function barHeight(percent: number | null | undefined): number {
  return Math.round(clampPercent(percent) * 10) / 10
}

/** The thinnest bar drawn (so a result of 0 % is a visible stub, not an empty gap). */
export const MIN_BAR_PERCENT = 2

/** The CSS height of a bar. */
export const barCss = (percent: number | null | undefined): string => `${Math.max(MIN_BAR_PERCENT, barHeight(percent))}%`

/** The accessible name of one bar: the exam and its percent ("Algebra 1: 85%"). */
export function barLabel(p: Pick<StatsTrendPoint, 'exam_title' | 'percent'>): string {
  return `${p.exam_title}: ${fmtPercent(p.percent) ?? '0'}%`
}

export type TrendDirection = 'none' | 'up' | 'down' | 'steady'

/**
 * Where the recent results are heading, in words (never colour alone): needs three attempts; compares the average of the
 * later half with the earlier half, a change of five points or more counts.
 */
export function trendDirection(points: readonly Pick<StatsTrendPoint, 'percent'>[]): TrendDirection {
  if (points.length < 3) return 'none'
  const half = Math.floor(points.length / 2)
  const mean = (xs: readonly Pick<StatsTrendPoint, 'percent'>[]) => xs.reduce((a, x) => a + x.percent, 0) / xs.length
  const delta = mean(points.slice(points.length - half)) - mean(points.slice(0, half))
  return delta >= 5 ? 'up' : delta <= -5 ? 'down' : 'steady'
}

/** The subject has results to show (a subject whose attempts all wait for grading or an embargo has none yet). */
export const subjectHasResults = (s: Pick<SubjectStats, 'attempts' | 'average_percent'>): boolean => s.attempts > 0 && s.average_percent !== null

/** No subject has any counted attempt: the page shows its empty state. */
export function statsEmpty(s: StudentStats | null | undefined): boolean {
  if (!s) return false
  return (s.overall?.attempts ?? 0) === 0 && !(s.subjects ?? []).some(subjectHasResults)
}

/** The subjects with results first (most attempts first), then those without. */
export function orderSubjects(subjects: readonly SubjectStats[]): SubjectStats[] {
  return [...subjects].sort((a, b) => Number(subjectHasResults(b)) - Number(subjectHasResults(a)) || b.attempts - a.attempts || a.subject_name.localeCompare(b.subject_name))
}
