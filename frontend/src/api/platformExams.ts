import { platformFetch } from '@/lib/platformApi'
import type { HiddenReason } from './platformContent'

export interface AssessmentInfo {
  /** null for exams created by an admin */
  id: string; teacher_id: string | null; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; question_count: number; total_points: number
  duration_min: number | null; opens_at: number | null; closes_at: number | null
  max_attempts: number; show_answers: boolean; status: 'draft' | 'published' | 'closed' | 'archived'
  attempts_used: number; attempt_count: number
  shuffle_questions: boolean; shuffle_options: boolean; pass_mark: number | null
  release_mode: 'immediate' | 'after_close'; closed_at: number | null; archived_at: number | null; created_by: string | null
  /** only on the owner's list (`/assessments/mine`): whether students can see this exam, and if not why not */
  visible?: boolean
  hidden_reason?: HiddenReason | null
}
export interface MyAttempt {
  attempt_id: string; assessment_id: string; title: string
  status: 'in_progress' | 'submitted' | 'expired'; score: number; total: number; pending: number; started_at: number
  /** false while the score is withheld until the exam closes */
  released: boolean; passed: boolean | null
}
export interface AssessmentDetail extends AssessmentInfo {
  can_start: boolean; in_progress_attempt: string | null; attempts: MyAttempt[]
  /** staff only (a student always gets false): a teacher's exam closed/archived by someone other than its owner */
  locked?: boolean
}
export interface PublicQuestion { id: string; type: string; stem: string; options: string[]; points: number }
export interface StartRes {
  attempt_id: string; started_at: number; ends_at: number; questions: PublicQuestion[]; resumed: boolean
  /** autosaved answers, in the displayed option letters */
  saved_answers: Record<string, string>; saved_at: number | null
}
export interface ItemResult {
  id: string; type: string; stem: string; options: string[]; your_answer: string | null
  correct: boolean | null; points: number; max: number; correct_answer: string | null; analysis: string | null
}
export interface AttemptResult {
  attempt_id: string; assessment_id: string; title: string; student_name: string
  status: 'submitted' | 'expired'; score: number; total: number; pending: number
  started_at: number; submitted_at: number | null; show_answers: boolean; items: ItemResult[] | null
  released: boolean; release_at: number | null; pass_mark: number | null; passed: boolean | null
  /** only graders/admins receive it */
  tab_leaves: number | null
}
export interface AttemptRow { attempt_id: string; student_name: string; status: string; score: number; pending: number; submitted_at: number | null; tab_leaves: number; passed: boolean | null }
export interface ResultsSummary {
  info: AssessmentInfo; submitted: number; average: number; highest: number; lowest: number; passed: number; attempts: AttemptRow[]
  /** the caller owns this exam and it is neither archived nor locked: the answer key can be corrected */
  can_correct: boolean
}

// Exams are written through the exam builder's API (`platformExamAdmin.ts`: /admin/exams and /teacher/exams); the legacy
// POST/PATCH/DELETE /assessments routes are no longer called from the app. Reads stay here.
export const getAssessment = (id: string) => platformFetch<AssessmentDetail>(`/assessments/${id}`)
export const startAssessment = (id: string) => platformFetch<StartRes>(`/assessments/${id}/start`, { method: 'POST', body: {} })
export const submitAttempt = (id: string, answers: Record<string, string>) =>
  platformFetch<AttemptResult>(`/attempts/${id}/submit`, { body: { answers } })
/** Autosave of the in-progress answers (displayed letters). */
export const saveAnswers = (id: string, answers: Record<string, string>) => platformFetch<{ saved_at: number }>(`/attempts/${id}/answers`, { method: 'PUT', body: { answers } })
/** Integrity event; informational only (graders see the count). */
export const sendAttemptEvent = (id: string, type: 'tab_leave') => platformFetch<void>(`/attempts/${id}/events`, { method: 'POST', body: { type } })
export const getAttempt = (id: string) => platformFetch<AttemptResult>(`/attempts/${id}`)
export const gradeAttempt = (id: string, grades: Record<string, number>) =>
  platformFetch<AttemptResult>(`/attempts/${id}/grade`, { method: 'PATCH', body: { grades } })
export const assessmentResults = (id: string) => platformFetch<ResultsSummary>(`/assessments/${id}/results`)
export const subjectAssessments = (id: string) => platformFetch<AssessmentInfo[]>(`/subjects/${id}/assessments`)
export const myAssessments = () => platformFetch<AssessmentInfo[]>('/assessments/mine')
export const availableAssessments = () => platformFetch<AssessmentInfo[]>('/assessments/available')
export const myAttempts = () => platformFetch<MyAttempt[]>('/my/attempts')
