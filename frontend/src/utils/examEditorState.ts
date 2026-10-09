// Which parts of the shared exam editor (admin and teacher) can be changed, and which notice explains why not. Pure, so a
// plain script can test it; the component only reads the result.

export type EditorMode = 'admin' | 'teacher'
export type EditorStatus = 'draft' | 'published' | 'closed' | 'archived'
/** `teacher-owned` = an admin looking at a teacher's exam, `locked` = closed/archived by someone else, `archived` = the owner's own archive, `frozen` = students have started. */
export type EditorNotice = 'teacher-owned' | 'locked' | 'archived' | 'frozen' | null

/** The fields of an exam detail that decide what the editor allows. */
export interface EditorExam {
  status: EditorStatus
  attempt_count: number
  can_edit: boolean
  teacher_id: string | null
  /** the actor owns it (a server field; older answers without it fall back to "an admin's exam has no teacher") */
  owned?: boolean
  locked?: boolean
}

export interface EditorState {
  /** students have started: questions, scores, duration, opening time and shuffling are frozen */
  frozen: boolean
  archived: boolean
  /** nothing at all can be changed (not editable by this actor right now) */
  readOnly: boolean
  /** questions / duration / opening time / shuffling cannot change */
  contentLocked: boolean
  /** the closing time, attempts, answers display, pass mark and result timing cannot change */
  settingsLocked: boolean
  /** the "publish" button belongs to the form (a new or draft exam) */
  canPublish: boolean
  notice: EditorNotice
}

export function editorState(exam: EditorExam | null): EditorState {
  const frozen = (exam?.attempt_count ?? 0) > 0
  const archived = exam?.status === 'archived'
  const readOnly = !!exam && !exam.can_edit
  const owned = exam ? (exam.owned ?? exam.teacher_id === null) : true
  let notice: EditorNotice = null
  if (exam && readOnly) notice = !owned ? 'teacher-owned' : exam.locked ? 'locked' : archived ? 'archived' : null
  else if (frozen) notice = 'frozen'
  return {
    frozen,
    archived,
    readOnly,
    contentLocked: frozen || readOnly || archived,
    settingsLocked: readOnly || archived,
    canPublish: !exam || exam.status === 'draft',
    notice,
  }
}

/**
 * The admin's switch for exam creation (teachers only; the admin builder is never blocked by it): a NEW exam cannot be
 * saved at all, and a draft cannot be published, while it is off. A published or closed exam stays fully manageable.
 */
export function examsSwitch(mode: EditorMode, examsOff: boolean, exam: Pick<EditorExam, 'status'> | null): { createBlocked: boolean; publishBlocked: boolean } {
  if (mode !== 'teacher' || !examsOff) return { createBlocked: false, publishBlocked: false }
  return { createBlocked: !exam, publishBlocked: !exam || exam.status === 'draft' }
}
