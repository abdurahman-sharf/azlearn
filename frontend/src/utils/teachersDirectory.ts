import type { DirectoryTeacher, TeacherDirectoryQuery } from '@/api/platformLearning'

// Pure rules of the teachers directory page: the search / subject / page state kept in the address bar (same pattern as
// "My exams" and "My courses"), the request it makes, paging, and the optimistic follow toggle on a loaded list.

export const TEACHERS_PER = 24

export interface TeacherFilters {
  q: string
  subjectId: string
  /** zero-based */
  page: number
}
export const DEFAULT_TEACHER_FILTERS: TeacherFilters = { q: '', subjectId: '', page: 0 }

const first = (v: unknown): string => (Array.isArray(v) ? String(v[0] ?? '') : typeof v === 'string' ? v : '')

/** Reads the filters back from a route query; anything unknown or malformed falls back to the default. */
export function teacherFiltersFromQuery(query: Record<string, unknown>): TeacherFilters {
  const page = Number.parseInt(first(query.page), 10)
  return {
    q: first(query.q).slice(0, 100),
    subjectId: first(query.subject).slice(0, 64),
    page: Number.isFinite(page) && page > 0 ? Math.min(page - 1, 10_000) : 0,
  }
}

/** The route query for the filters: only what differs from the default, the page one-based. */
export function teacherFiltersToQuery(f: TeacherFilters): Record<string, string> {
  const out: Record<string, string> = {}
  if (f.q.trim()) out.q = f.q.trim()
  if (f.subjectId) out.subject = f.subjectId
  if (f.page > 0) out.page = String(f.page + 1)
  return out
}

/** The directory request for the filters. */
export function teacherListQuery(f: TeacherFilters, per = TEACHERS_PER): TeacherDirectoryQuery {
  const out: TeacherDirectoryQuery = { limit: per, offset: Math.max(0, f.page) * per }
  if (f.q.trim()) out.q = f.q.trim()
  if (f.subjectId) out.subject_id = f.subjectId
  return out
}

/** Number of pages for `total` rows (at least one, so an empty list still has "page 1 / 1"). */
export const teacherPages = (total: number, per = TEACHERS_PER): number => Math.max(1, Math.ceil(Math.max(0, total) / per))

/** The last page that exists for `total` rows. */
export const teacherClampPage = (page: number, total: number, per = TEACHERS_PER): number => Math.min(Math.max(0, page), teacherPages(total, per) - 1)

/** The list with one teacher's `following` flag set (a new array; the other rows are the same objects). */
export function withFollowing(items: readonly DirectoryTeacher[], id: string, following: boolean): DirectoryTeacher[] {
  return items.map((t) => (t.id === id ? { ...t, following } : t))
}

/** The filters differ from the default (a "no results" message offers to clear them). */
export const teacherFiltered = (f: TeacherFilters): boolean => !!f.q.trim() || !!f.subjectId
