import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'
import type { AssessmentInfo } from '@/api/platformExams'

export interface PendingExam {
  assessment_id: string; title: string; subject_name: string
  pending_attempts: number; pending_answers: number; oldest_submitted_at: number | null
}
export interface GradingAnswer { attempt_id: string; student_name: string; answer: string; points: number | null; submitted_at: number | null }
export interface GradingQuestion { id: string; stem: string; reference: string; max: number; pending: number; answers: GradingAnswer[] }
export interface GradingSheet { assessment_id: string; title: string; subject_name: string; questions: GradingQuestion[] }

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

export const pendingExams = () => platformFetch<PendingExam[]>('/grading/pending')
export const gradingSheet = (id: string, pending: boolean) => platformFetch<GradingSheet>(`/assessments/${id}/grading${qs({ pending })}`)
export const gradeBatch = (id: string, grades: { attempt_id: string; question_id: string; points: number }[]) =>
  platformFetch<{ updated: number }>(`/assessments/${id}/grade-batch`, { method: 'POST', body: { grades } })
export const examAnalytics = (id: string) => platformFetch<Analytics>(`/assessments/${id}/analytics`)
export const attemptsTable = (id: string, f: TableFilter) => platformFetch<{ items: StudentRow[]; total: number }>(`/assessments/${id}/attempts${qs({ ...f })}`)

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
