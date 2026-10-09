import type { ExamFilter, ExamRow, Phase } from '@/api/platformExamAdmin'

// Pure rules of the teacher's "My exams" page: the tabs and what each one asks the server for, the filters kept in the
// address bar, and which actions a row offers. Kept out of the view so a plain script can test them.

/** `admin` = the admin's exams for my approved subjects (read-only: results and grading). */
export type MyTab = 'all' | 'draft' | 'published' | 'closed' | 'archived' | 'admin'
export const MY_TABS: MyTab[] = ['all', 'draft', 'published', 'closed', 'archived', 'admin']
export const MY_PER = 25

export interface MyFilters {
  tab: MyTab
  q: string
  subjectId: string
  /** zero-based */
  page: number
}
export const DEFAULT_MY_FILTERS: MyFilters = { tab: 'all', q: '', subjectId: '', page: 0 }

/** The list request for the filters. `all` leaves the status out (the server then hides archived exams). */
export function listFilter(f: MyFilters, per = MY_PER): ExamFilter {
  const out: ExamFilter = { limit: per, offset: Math.max(0, f.page) * per }
  if (f.tab === 'admin') out.scope = 'admin'
  else if (f.tab !== 'all') out.status = f.tab as Phase
  if (f.q.trim()) out.q = f.q.trim()
  if (f.subjectId) out.subject_id = f.subjectId
  return out
}

const first = (v: unknown): string => (Array.isArray(v) ? String(v[0] ?? '') : typeof v === 'string' ? v : '')

/** Reads the filters back from a route query; anything unknown or malformed falls back to the default. */
export function filtersFromQuery(query: Record<string, unknown>): MyFilters {
  const tab = first(query.tab) as MyTab
  const page = Number.parseInt(first(query.page), 10)
  return {
    tab: MY_TABS.includes(tab) ? tab : 'all',
    q: first(query.q).slice(0, 100),
    subjectId: first(query.subject).slice(0, 64),
    page: Number.isFinite(page) && page > 0 ? Math.min(page - 1, 10_000) : 0,
  }
}

/** The route query for the filters: only what differs from the default, the page one-based. */
export function filtersToQuery(f: MyFilters): Record<string, string> {
  const out: Record<string, string> = {}
  if (f.q.trim()) out.q = f.q.trim()
  if (f.tab !== 'all') out.tab = f.tab
  if (f.subjectId) out.subject = f.subjectId
  if (f.page > 0) out.page = String(f.page + 1)
  return out
}

/** The last page that exists for `total` rows (the page after a delete may be gone). */
export function clampPage(page: number, total: number, per = MY_PER): number {
  return Math.min(Math.max(0, page), Math.max(1, Math.ceil(Math.max(0, total) / per)) - 1)
}

export type RowAction = 'edit' | 'publish' | 'unpublish' | 'close' | 'reopen' | 'archive' | 'restore' | 'duplicate' | 'delete' | 'results' | 'grading'

/** The fields of an exam row the actions depend on. */
export type ActionRow = Pick<ExamRow, 'status' | 'can_edit' | 'locked' | 'attempt_count' | 'pending_answers'>

export interface RowActions {
  /** the actions to render, in display order */
  shown: RowAction[]
  /** shown but unusable because the admin switched exam creation off (publishing and copying need it) */
  disabled: RowAction[]
  /** the exam has attempts, so "delete" is replaced by an explanation (close or archive instead) */
  deleteBlocked: boolean
}

/**
 * What a row of the teacher's list offers.
 * - the admin's exams (`readOnly`): results and grading only;
 * - a locked exam (closed/archived by someone else): copy, results and grading, and delete while nobody has taken it -
 *   every other action is refused;
 * - otherwise by the stored status. Reopening does not need the exam switch; publishing and copying do.
 * `status` is the stored one: a published exam past its closing time reads as closed but is closed with "close".
 */
export function rowActions(row: ActionRow, o: { examsOff: boolean; readOnly: boolean }): RowActions {
  const tail: RowAction[] = []
  if (row.attempt_count > 0) tail.push('results')
  if (row.pending_answers > 0) tail.push('grading')
  if (o.readOnly) return { shown: tail, disabled: [], deleteBlocked: false }

  const hasAttempts = row.attempt_count > 0
  const shown: RowAction[] = []
  if (row.locked) {
    // the owner may still remove an exam nobody sat (the server allows it); one with attempts keeps its results
    shown.push('duplicate', ...tail)
    if (!hasAttempts) shown.push('delete')
  } else {
    if (row.can_edit) shown.push('edit')
    switch (row.status) {
      case 'draft':
        shown.push('publish', 'duplicate', 'archive')
        break
      case 'published':
        if (!hasAttempts) shown.push('unpublish')
        shown.push('close', 'duplicate')
        break
      case 'closed':
        shown.push('reopen', 'duplicate', 'archive')
        break
      case 'archived':
        shown.push('restore', 'duplicate')
        break
    }
    shown.push(...tail)
    if (!hasAttempts) shown.push('delete')
  }
  const disabled: RowAction[] = o.examsOff ? shown.filter((a) => a === 'publish' || a === 'duplicate') : []
  return { shown, disabled, deleteBlocked: !row.locked && hasAttempts }
}
