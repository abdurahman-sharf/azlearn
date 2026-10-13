import { platformFetch } from '@/lib/platformApi'

/** The caller's own numbers (teacher accounts only): every figure is scoped to the signed-in teacher by the server. */
export interface TeacherStats {
  teaching: { pending: number; approved: number; rejected: number }
  content: { posts_draft: number; posts_published: number; courses_draft: number; courses_published: number; live_upcoming: number }
  assessments: { draft: number; published: number; closed: number; attempts_submitted: number }
  /** written answers still waiting for this teacher's grade, and in how many exams */
  pending_grading: { answers: number; exams: number }
  unread: number
  /** false only when the admin explicitly turned exam creation off (an unset setting means enabled) */
  can_create_exams: boolean
}

export const teacherStats = () => platformFetch<TeacherStats>('/teacher/stats')
