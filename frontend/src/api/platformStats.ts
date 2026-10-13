import { platformFetch } from '@/lib/platformApi'

/** Platform-wide counters of the public landing page. */
export interface PublicStats {
  questions: number
  subjects: number
  attempts: number
}
/** Per-subject numbers for the admin cards (exams of the subject, submitted attempts, distinct questions). */
export interface SubjectStat {
  subject_id: string
  institution_id: string
  unit_id: string | null
  is_active: boolean
  exams: number
  attempts: number
  questions: number
}
export interface InstitutionStat {
  institution_id: string
  subjects: number
  exams: number
  attempts: number
  questions: number
}

export const publicStats = () => platformFetch<PublicStats>('/public/stats')
export const institutionStats = () => platformFetch<InstitutionStat[]>('/admin/stats/institutions')
export const subjectStats = (institutionId?: string) =>
  platformFetch<SubjectStat[]>(`/admin/stats/subjects${institutionId ? `?institution_id=${encodeURIComponent(institutionId)}` : ''}`)
