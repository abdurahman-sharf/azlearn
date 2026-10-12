import { platformFetch } from '@/lib/platformApi'
import type { Phase } from './platformExamAdmin'

// The student's own screens (phase 4-1). Every figure is computed by the server from the session user; nothing here
// sends a student id or a role.

/** What one exam is for the signed-in student (one server rule, shared by "My exams", the overview and the exam page). */
export type StudentExamState = 'in_progress' | 'open' | 'upcoming' | 'done' | 'missed'
/** Why the exam cannot be started now (null when it can, or while an attempt is in progress). */
export type StudentExamReason = 'not_enrolled' | 'not_open' | 'closed' | 'no_attempts'

/** The student's best submitted attempt. score/percent/passed are null while the result is withheld (`released` false). */
export interface BestAttempt {
  attempt_id: string
  score: number | null
  total: number
  percent: number | null
  passed: boolean | null
  released: boolean
  /** written answers of this attempt still waiting for a grade (null while withheld) */
  pending?: number | null
}

export interface StudentExamRow {
  id: string
  title: string
  subject_id: string
  subject_name: string
  teacher_name: string
  question_count: number
  total_points: number
  duration_min: number | null
  opens_at: number | null
  closes_at: number | null
  max_attempts: number
  pass_mark: number | null
  release_mode: 'immediate' | 'after_close'
  /** the effective phase (a published exam past its closing time reads as closed) */
  phase: Phase
  state: StudentExamState
  can_start: boolean
  reason: StudentExamReason | null
  attempts_used: number
  in_progress_attempt: { attempt_id: string; ends_at: number } | null
  best: BestAttempt | null
  /** newest attempt of any status */
  last_attempt_id: string | null
  /** false: the exam is no longer shown to students (hidden subject, archived, unenrolled); the row only leads to my own results */
  visible: boolean
}

export type StudentExamTab = 'all' | StudentExamState
export type StudentExamCounts = Record<StudentExamState, number>
export interface StudentExamList { items: StudentExamRow[]; total: number; counts: StudentExamCounts }
export interface StudentExamQuery { tab?: StudentExamTab; subject_id?: string; q?: string; limit?: number; offset?: number }

export function myExams(f: StudentExamQuery = {}): Promise<StudentExamList> {
  const p = new URLSearchParams()
  if (f.tab && f.tab !== 'all') p.set('tab', f.tab)
  if (f.subject_id) p.set('subject_id', f.subject_id)
  if (f.q) p.set('q', f.q)
  if (f.limit !== undefined) p.set('limit', String(f.limit))
  if (f.offset) p.set('offset', String(f.offset))
  const qs = p.toString()
  return platformFetch(`/my/exams${qs ? `?${qs}` : ''}`)
}

export interface StudentOverview {
  enrolled: number
  unread: number
  /** an attempt still running: the overview's first card */
  resume: { attempt_id: string; assessment_id: string; title: string; ends_at: number } | null
  exams: {
    open: number
    upcoming: number
    in_progress: number
    /** startable now and closing within 48 hours, soonest first (at most 5) */
    closing_soon: { id: string; title: string; subject_name: string; closes_at: number }[]
  }
  /** my submitted attempts that still wait for a teacher's grade */
  awaiting_grading: number
  /** newest 3 submitted attempts; score/percent/passed are null while withheld */
  recent_results: {
    attempt_id: string; assessment_id: string; title: string
    score: number | null; total: number; percent: number | null; passed: boolean | null; released: boolean; submitted_at: number
    /** written answers still waiting for a grade (null while withheld) */
    pending?: number | null
  }[]
  /** started and not finished, newest activity first (at most 4) */
  courses: { course_id: string; title: string; subject_name: string; percent: number; next_lesson_id: string | null }[]
  next_live: { id: string; title: string; subject_id: string; subject_name: string; starts_at: number; duration_min: number } | null
  checklist: { placement: boolean; enrolled: boolean }
}

export const studentOverview = () => platformFetch<StudentOverview>('/student/overview')

export interface SuggestedSubject {
  id: string
  name_ar: string
  name_en: string | null
  /** unit names from the institution's root down to the subject's unit */
  unit_path: string[]
  enrolled: boolean
}
export interface SuggestedSubjects {
  placement: { institution_id: string; institution_name: string; unit_id: string | null; unit_path: string[] } | null
  subjects: SuggestedSubject[]
}

/** The active subjects of my level (and the levels below it) in the institution I placed myself in. */
export const suggestedSubjects = () => platformFetch<SuggestedSubjects>('/me/suggested-subjects')

/** At most this many subjects per bulk enrolment request (the server answers 400 `invalid_selection` otherwise). */
export const BULK_ENROLL_MAX = 30
/** Each id goes through the single-enrolment rules; one that fails a rule or is already enrolled is counted as skipped. */
export const bulkEnroll = (subject_ids: string[]) =>
  platformFetch<{ enrolled: number; skipped: number }>('/enrollments/bulk', { body: { subject_ids } })
