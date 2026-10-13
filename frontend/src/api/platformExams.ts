import { platformFetch } from '@/lib/platformApi'
import type { HiddenReason } from './platformContent'
import type { Phase } from './platformExamAdmin'
import type { StudentExamReason, StudentExamState } from './platformStudent'

export interface AssessmentInfo {
  /** null for exams created by an admin */
  id: string; teacher_id: string | null; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; question_count: number; total_points: number
  duration_min: number | null; opens_at: number | null; closes_at: number | null
  max_attempts: number; show_answers: boolean; status: 'draft' | 'published' | 'closed' | 'archived'
  attempts_used: number; attempt_count: number
  shuffle_questions: boolean; shuffle_options: boolean; pass_mark: number | null
  release_mode: 'immediate' | 'after_close'; closed_at: number | null; archived_at: number | null; created_by: string | null
  /** practice attempts are on: enrolled students can open the questions WITH their answers (not graded, no attempt used) */
  allow_practice?: boolean
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
  /** student callers only (absent or null for other roles): this exam's state for me, why I cannot start it, the effective phase */
  state?: StudentExamState | null
  reason?: StudentExamReason | null
  phase?: Phase | null
  /** the server's clock (epoch ms) when this answer was written; the page measures its own offset from it */
  server_now?: number
}
export interface PublicQuestion { id: string; type: string; stem: string; options: string[]; points: number }
export interface StartRes {
  attempt_id: string; started_at: number
  /** the effective end: the duration, or the closing time when that comes first (a hard end) */
  ends_at: number
  questions: PublicQuestion[]; resumed: boolean
  /** autosaved answers, in the displayed option letters */
  saved_answers: Record<string, string>; saved_at: number | null
  /** the server's clock (epoch ms) when this answer was written */
  server_now?: number
  /** it is the closing time, not the duration, that ends this attempt */
  limited_by_closing?: boolean
}
export interface ItemResult {
  id: string; type: string; stem: string; options: string[]; your_answer: string | null
  correct: boolean | null; points: number; max: number; correct_answer: string | null; analysis: string | null
  /** the grader's comment on a written answer (the attempt's own student sees it once the result is released) */
  feedback?: string | null
  /** graders and admins only (null for a student): who wrote the grade, and when */
  graded_by_name?: string | null
  graded_at?: number | null
  /** the text of the chosen options (original order, joined with " | "); the answer itself for fill-blank / short; null when unanswered */
  your_answer_text?: string | null
  chapter?: string | null
  difficulty?: string | null
}
export interface AttemptResult {
  attempt_id: string; assessment_id: string; title: string; student_name: string
  status: 'submitted' | 'expired' | 'in_progress'; score: number; total: number; pending: number
  started_at: number; submitted_at: number | null; show_answers: boolean; items: ItemResult[] | null
  released: boolean; release_at: number | null; pass_mark: number | null; passed: boolean | null
  /** only graders/admins receive it */
  tab_leaves: number | null
  /** the attempt's student and graders: which attempt of the exam this is (1-based, by start time) */
  attempt_no?: number
  /** this is the student's best fully graded, released attempt of the exam (ties go to the newest) */
  is_best?: boolean
  /** seconds from start to hand-in; null while running */
  duration_sec?: number | null
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
export interface SaveRes {
  saved_at: number
  /** the server's clock (epoch ms) */
  server_now?: number
  /** how many answers the server now holds for this attempt */
  answered?: number
  /** the attempt's end as the server derives it NOW: it moves when the owner extends the closing time */
  ends_at?: number
  limited_by_closing?: boolean
}
/**
 * Autosave of the in-progress answers (displayed letters). By default the server MERGES the keys sent into what it holds
 * (an empty string removes a key), so two devices saving different questions never erase each other; `replace` keeps the
 * old "this is everything" behaviour.
 */
export const saveAnswers = (id: string, answers: Record<string, string>, opts: { replace?: boolean } = {}) =>
  platformFetch<SaveRes>(`/attempts/${id}/answers`, { method: 'PUT', body: opts.replace ? { answers, replace: true } : { answers } })
/**
 * The same merge, sent as the page is being left: `keepalive` lets the browser finish the request after the page is gone.
 * Best effort - it never throws.
 */
export async function flushAnswers(id: string, answers: Record<string, string>): Promise<boolean> {
  try {
    await platformFetch<SaveRes>(`/attempts/${id}/answers`, { method: 'PUT', body: { answers }, keepalive: true })
    return true
  } catch {
    return false
  }
}
/** Integrity event; informational only (graders see the count). */
export const sendAttemptEvent = (id: string, type: 'tab_leave') => platformFetch<void>(`/attempts/${id}/events`, { method: 'POST', body: { type } })
export const getAttempt = (id: string) => platformFetch<AttemptResult>(`/attempts/${id}`)
/** Grades and/or comments on the written answers of one attempt, both keyed by question id. */
export const gradeAttempt = (id: string, body: { grades?: Record<string, number>; feedback?: Record<string, string> }) =>
  platformFetch<AttemptResult>(`/attempts/${id}/grade`, { method: 'PATCH', body })
export const assessmentResults = (id: string) => platformFetch<ResultsSummary>(`/assessments/${id}/results`)
export const subjectAssessments = (id: string) => platformFetch<AssessmentInfo[]>(`/subjects/${id}/assessments`)
export const myAssessments = () => platformFetch<AssessmentInfo[]>('/assessments/mine')
export const availableAssessments = () => platformFetch<AssessmentInfo[]>('/assessments/available')
export const myAttempts = () => platformFetch<MyAttempt[]>('/my/attempts')

// ── practice mode (phase 4-3): the questions WITH their answers, for an enrolled student, when the teacher allows it ──────
export interface PracticeQuestion {
  id: string; type: string; stem: string; options: string[]
  /** points the question is worth in the real exam (0 = cancelled by the teacher) */
  points: number
  /** the model answer exactly as stored (letters; a fill-blank answer keeps its `a|b` alternatives) */
  answer: string
  analysis: string | null
  chapter: string | null
  difficulty: string | null
}
export interface PracticeExam {
  id: string; title: string; subject_id: string; subject_name: string
  question_count: number; total_points: number
  questions: PracticeQuestion[]
}
/** Writes nothing on the server: no attempt, no score, no counters. 403 `practice_disabled` when the teacher has not allowed it. */
export const practiceExam = (id: string) => platformFetch<PracticeExam>(`/assessments/${encodeURIComponent(id)}/practice`)
