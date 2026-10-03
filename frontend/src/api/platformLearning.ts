import { platformFetch } from '@/lib/platformApi'

export interface Teaching {
  teacher_id: string
  teacher_name: string
  subject_id: string
  subject_name: string
  institution_id: string
  institution_name: string
  status: 'pending' | 'approved' | 'rejected'
}
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

export const updateProfile = (b: { full_name?: string; bio?: string }) =>
  platformFetch<void>('/me/profile', { method: 'PATCH', body: b })

export const getPlacement = () => platformFetch<Placement | null>('/me/placement')
export const setPlacement = (b: Placement) => platformFetch<void>('/me/placement', { method: 'PUT', body: b })

export const myTeaching = () => platformFetch<Teaching[]>('/teaching')
export const requestTeaching = (subject_id: string) => platformFetch<void>('/teaching', { body: { subject_id } })
export const dropTeaching = (subject_id: string) => platformFetch<void>(`/teaching/${subject_id}`, { method: 'DELETE' })
export const adminTeaching = (status?: string) => platformFetch<Teaching[]>(`/admin/teaching${status ? `?status=${status}` : ''}`)
export const decideTeaching = (b: { teacher_id: string; subject_id: string; status: 'approved' | 'rejected' | 'pending' }) =>
  platformFetch<void>('/admin/teaching', { method: 'PATCH', body: b })

export const myEnrollments = () => platformFetch<EnrolledSubject[]>('/enrollments')
export const enroll = (subject_id: string) => platformFetch<void>('/enrollments', { body: { subject_id } })
export const unenroll = (subject_id: string) => platformFetch<void>(`/enrollments/${subject_id}`, { method: 'DELETE' })

export const listTeachers = (q?: string) => platformFetch<TeacherCard[]>(`/teachers${q ? `?q=${encodeURIComponent(q)}` : ''}`)
export const getTeacher = (id: string) => platformFetch<TeacherPage>(`/teachers/${id}`)
export const follow = (id: string) => platformFetch<void>(`/teachers/${id}/follow`, { method: 'POST', body: {} })
export const unfollow = (id: string) => platformFetch<void>(`/teachers/${id}/follow`, { method: 'DELETE' })
export const getSubject = (id: string) => platformFetch<SubjectPage>(`/subjects/${id}`)
