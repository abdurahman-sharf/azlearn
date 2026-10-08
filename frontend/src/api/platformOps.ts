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
export interface Hit { id: string; title: string; subtitle: string; link: string }
export interface SearchResults { subjects: Hit[]; teachers: Hit[]; courses: Hit[]; posts: Hit[] }

export const report = (b: { target_type: ReportTarget; target_id: string; reason: string }) =>
  platformFetch<void>('/reports', { body: b })
export const adminReports = (status = 'open') => platformFetch<ReportRow[]>(`/admin/reports?status=${status}`)
export const resolveReport = (id: string, b: { status: 'resolved' | 'dismissed'; note?: string }) =>
  platformFetch<void>(`/admin/reports/${id}`, { method: 'PATCH', body: b })
export const adminStats = () => platformFetch<Stats>('/admin/stats')
export const adminAudit = (before?: number) => platformFetch<AuditRow[]>(`/admin/audit${before ? `?before=${before}` : ''}`)
export const search = (q: string) => platformFetch<SearchResults>(`/search?q=${encodeURIComponent(q)}`)
export const changePassword = (current: string, next: string) =>
  platformFetch<void>('/me/password', { body: { current, new: next } })
export const deleteAccount = (password: string) => platformFetch<void>('/me', { method: 'DELETE', body: { password } })

/** What deleting the caller's account would remove (the caller's own rows only; attempts = students' attempts on the caller's exams). */
export interface DeletionImpact { exams: number; attempts: number; courses: number; posts: number; live: number }
export const deletionImpact = () => platformFetch<DeletionImpact>('/me/deletion-impact')
