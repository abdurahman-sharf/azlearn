import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'
import type { Question } from '@exameow/shared'
import type { AssessmentInfo } from '@/api/platformExams'
import type { HiddenReason } from '@/api/platformContent'

export type Phase = 'draft' | 'published' | 'closed' | 'archived'
export const PHASES: Phase[] = ['draft', 'published', 'closed', 'archived']

export interface ExamRow extends AssessmentInfo {
  /** effective phase: a published exam past its closing time reads as closed */
  phase: Phase
  /** the actor may change this exam's content/settings right now (owned, not archived, not locked) */
  can_edit: boolean
  /** the actor is the owner: the teacher who made it, or an admin looking at an admin-made exam */
  owned: boolean
  /** a teacher's exam that an admin (or the system's report auto-hide) closed/archived: its owner may not undo that */
  locked: boolean
  /** submitted attempts, and written answers still waiting for a grade over them */
  submitted: number
  pending_answers: number
  /** whether students can see it right now, and if not why not (same rule as `/assessments/mine`) */
  visible: boolean
  hidden_reason: HiddenReason | null
}
export interface ExamDetail extends ExamRow {
  questions: Question[]
  sources: Record<string, string>
}
export interface ExamFilter {
  subject_id?: string; status?: Phase | 'all'; q?: string; limit?: number; offset?: number
  /** teachers only: `mine` (default) or `admin` = the admin's exams for my approved subjects (read-only) */
  scope?: 'mine' | 'admin'
}

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

export type ExamAction = 'publish' | 'unpublish' | 'close' | 'reopen' | 'archive' | 'restore' | 'unlock' | 'duplicate'
export type ExamScope = 'admin' | 'teacher'

/** Body of an answer-key correction: `set` a new answer/score/analysis, or `void` the question for everybody. */
export interface CorrectBody {
  mode: 'set' | 'void'
  answer?: string
  score?: number
  analysis?: string
  /** compute the effect without writing anything */
  dry_run?: boolean
}
export interface CorrectResult {
  mode: 'set' | 'void'
  question: Question
  old_total: number
  new_total: number
  /** submitted attempts that were re-evaluated */
  attempts_total: number
  affected_attempts: number
  affected_students: number
  up: number
  down: number
  unchanged: number
  changed_outcomes: number
  dry_run: boolean
}

/** The calls of one exam API: the admin's (`/admin/exams`) and the owner-teacher's (`/teacher/exams`) are identical in shape. */
export interface ExamApi {
  list: (f: ExamFilter) => Promise<{ items: ExamRow[]; total: number }>
  get: (id: string) => Promise<ExamDetail>
  create: (b: ExamInput) => Promise<ExamDetail>
  update: (id: string, b: ExamInput) => Promise<ExamDetail>
  action: (id: string, action: ExamAction) => Promise<ExamDetail>
  /** admins pass the typed title when attempts exist; teachers have no such override (the server answers 409 has_attempts) */
  remove: (id: string, confirmTitle?: string) => Promise<void>
  /** the same call under the contract's name */
  delete: (id: string, confirmTitle?: string) => Promise<void>
  correctQuestion: (id: string, questionId: string, body: CorrectBody) => Promise<CorrectResult>
}

export function examApi(scope: ExamScope): ExamApi {
  const base = scope === 'admin' ? '/admin/exams' : '/teacher/exams'
  const remove: ExamApi['remove'] = (id, confirmTitle) => platformFetch<void>(`${base}/${id}${scope === 'admin' ? qs({ confirm_title: confirmTitle }) : ''}`, { method: 'DELETE' })
  return {
    list: (f) => platformFetch<{ items: ExamRow[]; total: number }>(`${base}${qs({ ...f, scope: scope === 'teacher' ? f.scope : undefined })}`),
    get: (id) => platformFetch<ExamDetail>(`${base}/${id}`),
    create: (b) => platformFetch<ExamDetail>(base, { method: 'POST', body: b }),
    update: (id, b) => platformFetch<ExamDetail>(`${base}/${id}`, { method: 'PATCH', body: b }),
    action: (id, action) => platformFetch<ExamDetail>(`${base}/${id}/${action}`, { method: 'POST', body: {} }),
    remove,
    delete: remove,
    correctQuestion: (id, questionId, body) => platformFetch<CorrectResult>(`${base}/${id}/questions/${encodeURIComponent(questionId)}/correct`, { method: 'POST', body }),
  }
}

export const adminExams = examApi('admin')
export const teacherExams = examApi('teacher')
/** The API of the signed-in role (the answer-key dialog is shared by both). */
export const examApiFor = (role: string | null | undefined): ExamApi => (role === 'admin' ? adminExams : teacherExams)

// Admin shortcuts kept for the admin screens.
export const listExams = adminExams.list
export const getExam = adminExams.get
export const createExam = adminExams.create
export const updateExam = adminExams.update
export const examAction = adminExams.action
export const deleteExam = adminExams.remove

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
