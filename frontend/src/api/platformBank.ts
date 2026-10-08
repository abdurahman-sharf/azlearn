import { platformFetch } from '@/lib/platformApi'

export type BankType = 'single_choice' | 'multi_choice' | 'true_false' | 'fill_blank' | 'short_answer'
export type BankDifficulty = 'easy' | 'medium' | 'hard'
export const BANK_TYPES: BankType[] = ['single_choice', 'multi_choice', 'true_false', 'fill_blank', 'short_answer']
export const BANK_DIFFICULTIES: BankDifficulty[] = ['easy', 'medium', 'hard']

export interface BankItem {
  id: string
  subject_id: string
  type: BankType
  stem: string
  options: string[]
  answer: string
  analysis: string
  chapter: string | null
  difficulty: BankDifficulty | null
  tags: string[]
  created_by: string | null
  created_at: number
  updated_at: number
  archived_at: number | null
}
export interface BankItemInput {
  type: BankType
  stem: string
  options: string[]
  answer: string
  analysis: string
  chapter: string | null
  difficulty: BankDifficulty | null
  tags: string[]
}
export interface BankFilter {
  subject_id?: string
  type?: string
  difficulty?: string
  chapter?: string
  tag?: string
  q?: string
  state?: 'active' | 'archived' | 'all'
  limit?: number
  offset?: number
}
export interface Counted { value: string; count: number }
export interface Facets {
  active: number
  archived: number
  chapters: Counted[]
  tags: Counted[]
  types: Counted[]
  difficulties: Counted[]
}
export interface ImportResult {
  created: number
  skipped_duplicates: number
  duplicates_kept: number
  rejected: { index: number; error: string }[]
}

const qs = (o: Record<string, string | number | undefined>) => {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== '') p.set(k, String(v))
  const s = p.toString()
  return s ? `?${s}` : ''
}

export const listBank = (f: BankFilter) => platformFetch<{ items: BankItem[]; total: number }>(`/admin/bank${qs({ ...f })}`)
export const bankFacets = (subject_id: string) => platformFetch<Facets>(`/admin/bank/facets${qs({ subject_id })}`)
export const createBankItem = (subject_id: string, item: BankItemInput) =>
  platformFetch<{ item: BankItem; duplicate_of: string | null }>('/admin/bank', { method: 'POST', body: { subject_id, ...item } })
export const updateBankItem = (id: string, item: BankItemInput) =>
  platformFetch<{ item: BankItem; duplicate_of: string | null }>(`/admin/bank/${id}`, { method: 'PATCH', body: item })
export const bulkBank = (ids: string[], action: 'archive' | 'restore' | 'delete') =>
  platformFetch<{ changed: number }>('/admin/bank/bulk', { method: 'POST', body: { ids, action } })

/** Sends the items in batches the server accepts (500 per request) and sums the results. */
export async function importBank(subject_id: string, items: unknown[], skip_duplicates: boolean): Promise<ImportResult> {
  const total: ImportResult = { created: 0, skipped_duplicates: 0, duplicates_kept: 0, rejected: [] }
  for (let at = 0; at < items.length; at += 500) {
    const r = await platformFetch<ImportResult>('/admin/bank/import', { method: 'POST', body: { subject_id, items: items.slice(at, at + 500), skip_duplicates } })
    total.created += r.created
    total.skipped_duplicates += r.skipped_duplicates
    total.duplicates_kept += r.duplicates_kept
    total.rejected.push(...r.rejected.map((x) => ({ ...x, index: x.index + at })))
  }
  return total
}
