import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'
import type { Question } from '@exameow/shared'
import type { AssessmentInfo } from '@/api/platformExams'

export type Phase = 'draft' | 'published' | 'closed' | 'archived'
export const PHASES: Phase[] = ['draft', 'published', 'closed', 'archived']

export interface ExamRow extends AssessmentInfo {
  /** effective phase: a published exam past its closing time reads as closed */
  phase: Phase
  can_edit: boolean
}
export interface ExamDetail extends ExamRow {
  questions: Question[]
  sources: Record<string, string>
}
export interface ExamFilter { subject_id?: string; status?: Phase | 'all'; q?: string; limit?: number; offset?: number }

export interface ExamInput {
  subject_id?: string
  title?: string
  description?: string
  questions?: Question[]
  sources?: Record<string, string>
  duration_min?: number
  clear_duration?: boolean
  opens_at?: number
  clear_opens?: boolean
  closes_at?: number
  clear_closes?: boolean
  max_attempts?: number
  show_answers?: boolean
  shuffle_questions?: boolean
  shuffle_options?: boolean
  pass_mark?: number
  clear_pass_mark?: boolean
  release_mode?: 'immediate' | 'after_close'
  /** create only */
  status?: 'draft' | 'published'
}

const qs = (o: Record<string, string | number | undefined>) => {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== '') p.set(k, String(v))
  const s = p.toString()
  return s ? `?${s}` : ''
}

export const listExams = (f: ExamFilter) => platformFetch<{ items: ExamRow[]; total: number }>(`/admin/exams${qs({ ...f })}`)
export const getExam = (id: string) => platformFetch<ExamDetail>(`/admin/exams/${id}`)
export const createExam = (b: ExamInput) => platformFetch<ExamDetail>('/admin/exams', { method: 'POST', body: b })
export const updateExam = (id: string, b: ExamInput) => platformFetch<ExamDetail>(`/admin/exams/${id}`, { method: 'PATCH', body: b })
export type ExamAction = 'publish' | 'unpublish' | 'close' | 'reopen' | 'archive' | 'restore' | 'duplicate'
export const examAction = (id: string, action: ExamAction) => platformFetch<ExamDetail>(`/admin/exams/${id}/${action}`, { method: 'POST', body: {} })
export const deleteExam = (id: string, confirmTitle?: string) => platformFetch<void>(`/admin/exams/${id}${qs({ confirm_title: confirmTitle })}`, { method: 'DELETE' })

export interface GenerateParams {
  type_counts: Record<string, number>
  difficulty: 'easy' | 'medium' | 'hard'
  language: string
  auto_chapter: boolean
}
/** Cap-enforced generation with the platform's saved AI provider (one call counts against the daily caps). */
export async function generateQuestions(file: File, p: GenerateParams): Promise<Question[]> {
  const types = Object.entries(p.type_counts).filter(([, n]) => n > 0)
  const params = {
    question_types: types.map(([t]) => t),
    type_counts: Object.fromEntries(types),
    count: types.reduce((a, [, n]) => a + n, 0),
    difficulty: p.difficulty,
    language: p.language,
    auto_chapter: p.auto_chapter,
    source_name: file.name.replace(/\.[^.]+$/, ''),
  }
  const fd = new FormData()
  fd.append('params', JSON.stringify(params))
  fd.append('file', file)
  const token = getToken()
  const res = await fetch('/api/platform/admin/ai/generate', { method: 'POST', headers: token ? { Authorization: `Bearer ${token}` } : {}, body: fd }).catch(() => {
    throw new PlatformError('network', 0)
  })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new PlatformError((data as { error?: string }).error ?? (res.status === 413 ? 'invalid_file_size' : 'unknown'), res.status, data as Record<string, unknown>)
  return (data as { questions: Question[] }).questions
}
