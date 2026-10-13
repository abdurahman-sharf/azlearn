import type { MyTeaching, TeachingImpact } from '@/api/platformLearning'

// Pure rules of the teacher's "My subjects" page and of the pickers in the content editors.

export type TeachingGroupKey = 'approved' | 'pending' | 'rejected'
/** Order of the groups on the page: what the teacher can work with first, then what is waiting, then what was refused. */
export const GROUP_ORDER: TeachingGroupKey[] = ['approved', 'pending', 'rejected']

/**
 * Cards grouped by status, in GROUP_ORDER, empty groups left out. Waiting requests keep the order they were made in
 * (oldest first, like the admin's queue); decided ones show the most recent decision first.
 */
export function groupByStatus<T extends Pick<MyTeaching, 'status' | 'created_at' | 'decided_at' | 'subject_name'>>(rows: T[]): { status: TeachingGroupKey; rows: T[] }[] {
  const when = (r: T) => r.decided_at ?? r.created_at
  const cmp = (status: TeachingGroupKey) => (a: T, b: T) =>
    (status === 'pending' ? when(a) - when(b) : when(b) - when(a)) || a.subject_name.localeCompare(b.subject_name)
  return GROUP_ORDER.map((status) => ({ status, rows: rows.filter((r) => r.status === status).sort(cmp(status)) })).filter((g) => g.rows.length > 0)
}

/** subject id → status, for the SubjectPicker's `statusBySubject`. */
export function statusBySubject(rows: Pick<MyTeaching, 'subject_id' | 'status'>[]): Record<string, TeachingGroupKey> {
  const out: Record<string, TeachingGroupKey> = {}
  for (const r of rows) out[r.subject_id] = r.status
  return out
}

/** An approved assignment on a subject (and institution) that is switched on: the only rows content can be created for. */
export function isUsable(r: Pick<MyTeaching, 'status' | 'subject_active' | 'institution_active'>): boolean {
  return r.status === 'approved' && r.subject_active !== false && r.institution_active !== false
}

/** The rows the editors offer in their subject lists. */
export function usableSubjects<T extends Pick<MyTeaching, 'status' | 'subject_active' | 'institution_active'>>(rows: T[]): T[] {
  return rows.filter(isUsable)
}

/** "Institution › level › year" for a card; units are optional. */
export function placeLabel(institutionName: string, unitPath: string[] | undefined): string {
  return [institutionName, ...(unitPath ?? [])].filter((s) => s !== '').join(' › ')
}

/** How many items a withdrawal / revocation would hide. */
export const impactTotal = (i: TeachingImpact | null | undefined): number => (i ? i.posts + i.courses + i.live + i.exams : 0)
