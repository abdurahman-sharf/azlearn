import { platformFetch } from '@/lib/platformApi'

export type ReportTarget = 'post' | 'course' | 'live' | 'assessment' | 'review' | 'teacher'
export interface ReportRow {
  id: string; target_type: ReportTarget; target_id: string; title: string; link: string; reason: string
  reporter: string; status: 'open' | 'resolved' | 'dismissed'; note: string | null; created_at: number; open_reports_on_target: number
}
export interface Stats {
  users: Record<string, number>; pending_teachers: number; pending_teaching: number; pending_grading: number; open_reports: number; system_warnings: number
  signups_7d: number; institutions: Record<string, number>; subjects: number; attempts_submitted: number
  content: { posts: number; courses: number; lessons: number; live: number; assessments: number }
}
export interface AuditRow { id: number; actor: string | null; target_id: string | null; action: string; detail: string | null; created_at: number }
export interface Hit {
  id: string; title: string; subtitle: string; link: string
  /** up to 140 characters around the first match in the text field that matched (null when the title matched) */
  snippet?: string | null
  matched?: 'title' | 'text'
}
export type SearchGroupKey = 'subjects' | 'teachers' | 'courses' | 'posts' | 'live' | 'exams'
export type SearchCounts = Record<SearchGroupKey, number>
/** Without `group` each array holds up to 8 hits; with `group=g` only that array is filled (the others come back empty). */
export interface SearchResults {
  subjects: Hit[]; teachers: Hit[]; courses: Hit[]; posts: Hit[]
  /** live sessions and exams (exams only for students) - absent from an older server */
  live?: Hit[]; exams?: Hit[]
  /** the total number of matches per group */
  counts?: SearchCounts
}
export interface SearchOptions { group?: SearchGroupKey; limit?: number; offset?: number }

export const report = (b: { target_type: ReportTarget; target_id: string; reason: string }) =>
  platformFetch<void>('/reports', { body: b })
export const adminReports = (status = 'open') => platformFetch<ReportRow[]>(`/admin/reports?status=${status}`)
export const resolveReport = (id: string, b: { status: 'resolved' | 'dismissed'; note?: string }) =>
  platformFetch<void>(`/admin/reports/${id}`, { method: 'PATCH', body: b })
export const adminStats = () => platformFetch<Stats>('/admin/stats')
export const adminAudit = (before?: number) => platformFetch<AuditRow[]>(`/admin/audit${before ? `?before=${before}` : ''}`)
export function search(q: string, o: SearchOptions = {}): Promise<SearchResults> {
  const p = new URLSearchParams({ q })
  if (o.group) p.set('group', o.group)
  if (o.limit !== undefined) p.set('limit', String(o.limit))
  if (o.offset) p.set('offset', String(o.offset))
  return platformFetch<SearchResults>(`/search?${p.toString()}`)
}
export const changePassword = (current: string, next: string) =>
  platformFetch<void>('/me/password', { body: { current, new: next } })
/** `confirm_email` (the typed account e-mail) is required from a student who has taken exams (409 `confirm_required`). */
export const deleteAccount = (password: string, confirm_email?: string) =>
  platformFetch<void>('/me', { method: 'DELETE', body: { password, ...(confirm_email ? { confirm_email } : {}) } })

/**
 * What deleting the caller's account would remove (the caller's own rows only). Teacher figures: exams, attempts
 * (students' attempts on the caller's exams), courses, posts, live. Student figures (zero for others; optional while
 * an older server does not send them): the exams the student took, completed lessons, enrolments and reviews.
 */
export interface DeletionImpact {
  exams: number; attempts: number; courses: number; posts: number; live: number
  attempts_taken?: number; lessons_completed?: number; enrollments?: number; reviews?: number
}
export const deletionImpact = () => platformFetch<DeletionImpact>('/me/deletion-impact')
