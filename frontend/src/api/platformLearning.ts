import { platformFetch } from '@/lib/platformApi'
import type { InstitutionType } from '@/stores/auth'

export interface Teaching {
  teacher_id: string
  teacher_name: string
  subject_id: string
  subject_name: string
  institution_id: string
  institution_name: string
  status: 'pending' | 'approved' | 'rejected'
  /** only on the admin's list: the teacher's own account status (a request can come from a still-pending account) */
  teacher_status?: 'active' | 'pending' | 'rejected' | 'suspended'
}
/**
 * A row of `GET /teaching` (the teacher's own list): the public fields plus what only the teacher may see - when the
 * request was made and decided, the admin's note, where the subject sits, and whether it (or its institution) is
 * switched off. Students never receive these fields; the public teacher page uses the plain `Teaching`.
 */
export interface MyTeaching extends Teaching {
  created_at: number
  decided_at: number | null
  /** the admin's note with the decision (shown to the teacher only) */
  reason: string | null
  /** unit names from the root down to the subject's own unit; empty when the subject hangs on the institution */
  unit_path: string[]
  subject_active: boolean
  institution_active: boolean
}
/** A row of the admin's list: the same details plus the teacher's own account status. */
export interface AdminTeachingRow extends MyTeaching {
  teacher_status: 'active' | 'pending' | 'rejected' | 'suspended'
}
/** How many of a teacher's items on a subject are visible to students now and would stop being so (nothing is deleted). */
export interface TeachingImpact { posts: number; courses: number; live: number; exams: number }
export interface TeacherCard {
  id: string
  full_name: string
  bio: string | null
  subject_count: number
}
export interface TeacherPage {
  id: string
  full_name: string
  bio: string | null
  followers: number
  following: boolean
  subjects: Teaching[]
}
export interface SubjectPage {
  id: string
  name_ar: string
  name_en: string | null
  institution_id: string
  institution_name: string
  path: string[]
  enrolled: boolean
  enrolled_count: number
  teachers: TeacherCard[]
}
export interface EnrolledSubject {
  subject_id: string
  subject_name: string
  institution_id: string
  institution_name: string
}
export interface Placement {
  institution_id: string
  unit_id: string | null
}

export const updateProfile = (b: { full_name?: string; bio?: string; institution_type?: InstitutionType }) =>
  platformFetch<void>('/me/profile', { method: 'PATCH', body: b })

export const getPlacement = () => platformFetch<Placement | null>('/me/placement')
export const setPlacement = (b: Placement) => platformFetch<void>('/me/placement', { method: 'PUT', body: b })

export const myTeaching = () => platformFetch<MyTeaching[]>('/teaching')
export const requestTeaching = (subject_id: string) => platformFetch<void>('/teaching', { body: { subject_id } })
export const dropTeaching = (subject_id: string) => platformFetch<void>(`/teaching/${subject_id}`, { method: 'DELETE' })
/** What withdrawing from this subject would hide (the caller's own items only). */
export const teachingImpact = (subject_id: string) => platformFetch<TeachingImpact>(`/teaching/${subject_id}/impact`)

export type TeachingStatus = 'pending' | 'approved' | 'rejected'
export interface AdminTeachingQuery { status?: TeachingStatus; q?: string; limit?: number; offset?: number }
/** The admin's queue, oldest first within the status. */
export function adminTeaching(f: AdminTeachingQuery = {}): Promise<{ items: AdminTeachingRow[]; total: number }> {
  const p = new URLSearchParams()
  if (f.status) p.set('status', f.status)
  if (f.q) p.set('q', f.q)
  if (f.limit !== undefined) p.set('limit', String(f.limit))
  if (f.offset) p.set('offset', String(f.offset))
  const qs = p.toString()
  return platformFetch(`/admin/teaching${qs ? `?${qs}` : ''}`)
}
/** What revoking this teacher's assignment would hide. */
export const adminTeachingImpact = (teacher_id: string, subject_id: string) =>
  platformFetch<TeachingImpact>(`/admin/teaching/impact?teacher_id=${encodeURIComponent(teacher_id)}&subject_id=${encodeURIComponent(subject_id)}`)
/** `reason` (at most 300 characters) is the admin's note; the teacher sees it with the decision. */
export const decideTeaching = (b: { teacher_id: string; subject_id: string; status: TeachingStatus; reason?: string }) =>
  platformFetch<void>('/admin/teaching', { method: 'PATCH', body: b })

export const myEnrollments = () => platformFetch<EnrolledSubject[]>('/enrollments')
export const enroll = (subject_id: string) => platformFetch<void>('/enrollments', { body: { subject_id } })
export const unenroll = (subject_id: string) => platformFetch<void>(`/enrollments/${subject_id}`, { method: 'DELETE' })

export const listTeachers = (q?: string) => platformFetch<TeacherCard[]>(`/teachers${q ? `?q=${encodeURIComponent(q)}` : ''}`)
export const getTeacher = (id: string) => platformFetch<TeacherPage>(`/teachers/${id}`)
export const follow = (id: string) => platformFetch<void>(`/teachers/${id}/follow`, { method: 'POST', body: {} })
export const unfollow = (id: string) => platformFetch<void>(`/teachers/${id}/follow`, { method: 'DELETE' })
export const getSubject = (id: string) => platformFetch<SubjectPage>(`/subjects/${id}`)
