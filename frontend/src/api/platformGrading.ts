import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'
import type { AssessmentInfo } from '@/api/platformExams'

export interface PendingExam {
  assessment_id: string; title: string; subject_id: string; subject_name: string
  pending_attempts: number; pending_answers: number; oldest_submitted_at: number | null
  /** submitted attempts of the exam */
  attempts_total: number
  /** the caller owns the exam (false for an admin's exam graded by subject approval, or a teacher's exam seen by an admin) */
  owned: boolean
}
export interface GradingAnswer {
  attempt_id: string; student_name: string; answer: string; points: number | null; submitted_at: number | null
  /** the grader's comment on this answer, if any */
  feedback: string | null
  /** when the points were last written by a grader (null while waiting) */
  graded_at: number | null
}
export interface GradingQuestion { id: string; stem: string; reference: string; max: number; pending: number; answers: GradingAnswer[] }
export interface GradingSheet {
  assessment_id: string; title: string; subject_name: string
  /** false: the exam hides its per-question view from students, so they never see the comments written here */
  show_answers: boolean
  questions: GradingQuestion[]
}

export interface Bin { from: number; to: number; count: number }
export interface QStat {
  id: string; position: number; type: string; stem: string; max: number
  graded: number; answered: number; rate: number | null; avg_points: number | null; weak: boolean; easy: boolean
  /** the question was voided by an answer-key correction: it counts for nobody (max 0, no rate) */
  voided: boolean
}
export interface Analytics {
  info: AssessmentInfo
  enrolled: number; participants: number; participation_pct: number | null; attempts_submitted: number
  graded_students: number; awaiting_grading: number
  average: number | null; average_pct: number | null; median: number | null; highest: number | null; lowest: number | null
  passed: number; failed: number; distribution: Bin[]
  avg_duration_sec: number | null; tab_leave_students: number; tab_leaves_total: number
  questions: QStat[]
  /** also on `/results`; the results page reads it from there */
  can_correct?: boolean
}

/** One student's row of the "by student" view: best and last attempt, how many, what still waits. */
export interface StudentSummary {
  student_id: string; student_name: string; email: string | null; attempts: number
  best_score: number | null; best_percent: number | null; best_attempt_id: string | null
  last_attempt_id: string | null; last_status: 'submitted' | 'expired' | 'in_progress' | null; last_submitted_at: number | null
  /** pass flag of the best submitted attempt (null: no pass mark, nothing submitted, or still waiting for grading) */
  passed: boolean | null
  /** written answers still waiting for a grade, over all of the student's submitted attempts */
  pending: number
  tab_leaves: number
}
export type StudentSortKey = 'name' | 'best' | 'attempts' | 'last'
export type StudentResultFilter = '' | 'passed' | 'failed' | 'pending'
export interface StudentsFilter { sort?: StudentSortKey; dir?: 'asc' | 'desc'; result?: StudentResultFilter; q?: string; limit?: number; offset?: number }
/** Enrolled students who have no attempt at all on the exam (names only). */
export interface AbsentStudent { student_id: string; student_name: string }
export interface AbsentList { items: AbsentStudent[]; total: number; enrolled: number }
/** One answer's change in a `grade-batch` call: points, a comment, or both; a field that is absent is left as it is. */
export interface GradeBatchItem { attempt_id: string; question_id: string; points?: number; feedback?: string }

export type SortKey = 'name' | 'score' | 'duration' | 'tab_leaves' | 'submitted_at' | 'status'
export type ResultFilter = '' | 'passed' | 'failed' | 'pending' | 'expired' | 'in_progress'
export interface TableFilter { sort?: SortKey; dir?: 'asc' | 'desc'; result?: ResultFilter; q?: string; limit?: number; offset?: number }
export interface StudentRow {
  attempt_id: string; student_name: string; email: string | null; attempt_no: number; status: 'submitted' | 'expired' | 'in_progress'
  score: number; total: number; percent: number | null; passed: boolean | null; pending: number
  duration_sec: number | null; tab_leaves: number; started_at: number; submitted_at: number | null
}

const qs = (o: Record<string, string | number | boolean | undefined>) => {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== '') p.set(k, String(v))
  const s = p.toString()
  return s ? `?${s}` : ''
}

/** The exams with written answers waiting for the caller; `subjectId` narrows the list to one subject. */
export const pendingExams = (subjectId?: string) => platformFetch<PendingExam[]>(`/grading/pending${qs({ subject_id: subjectId })}`)
export const gradingSheet = (id: string, pending: boolean) => platformFetch<GradingSheet>(`/assessments/${id}/grading${qs({ pending })}`)
export const gradeBatch = (id: string, grades: GradeBatchItem[]) =>
  platformFetch<{ updated: number }>(`/assessments/${id}/grade-batch`, { method: 'POST', body: { grades } })
export const examAnalytics = (id: string) => platformFetch<Analytics>(`/assessments/${id}/analytics`)
export const attemptsTable = (id: string, f: TableFilter) => platformFetch<{ items: StudentRow[]; total: number }>(`/assessments/${id}/attempts${qs({ ...f })}`)
/** The same endpoint in its "by student" shape (`view=students`). */
export const studentsTable = (id: string, f: StudentsFilter) =>
  platformFetch<{ items: StudentSummary[]; total: number }>(`/assessments/${id}/attempts${qs({ view: 'students', ...f })}`)
export const absentStudents = (id: string, f: { q?: string; limit?: number; offset?: number } = {}) =>
  platformFetch<AbsentList>(`/assessments/${id}/absent${qs({ ...f })}`)

/** Downloads through fetch so the Bearer token is sent, then saves the blob (the server names the file in ASCII). */
export async function downloadExport(id: string, format: 'xlsx' | 'csv', part: 'results' | 'questions', lang: string): Promise<void> {
  const token = getToken()
  const res = await fetch(`/api/platform/assessments/${id}/export${qs({ format, part, lang })}`, { headers: token ? { Authorization: `Bearer ${token}` } : {} }).catch(() => {
    throw new PlatformError('network', 0)
  })
  if (!res.ok) throw new PlatformError(((await res.json().catch(() => ({}))) as { error?: string }).error ?? 'unknown', res.status)
  const m = /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')
  const url = URL.createObjectURL(await res.blob())
  const a = document.createElement('a')
  a.href = url
  a.download = m?.[1] ?? `results.${format}`
  a.style.display = 'none'
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}
