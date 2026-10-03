import { platformFetch } from '@/lib/platformApi'
import type { InstitutionType, Profile } from '@/stores/auth'

export interface Institution {
  id: string
  type: InstitutionType
  name_ar: string
  name_en: string | null
  city: string | null
  is_active: boolean
}
export type UnitKind = 'department' | 'level' | 'year' | 'term'
export interface Unit {
  id: string
  institution_id: string
  parent_id: string | null
  kind: UnitKind
  name_ar: string
  name_en: string | null
  sort_order: number
  is_active: boolean
}
export interface Subject {
  id: string
  institution_id: string
  unit_id: string | null
  name_ar: string
  name_en: string | null
  is_active: boolean
}
export interface Structure {
  institution: Institution
  units: Unit[]
  subjects: Subject[]
}

const qs = (o: Record<string, string | undefined>) => {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v) p.set(k, v)
  const s = p.toString()
  return s ? `?${s}` : ''
}

export const listUsers = (f: { status?: string; role?: string; q?: string }) =>
  platformFetch<Profile[]>(`/admin/users${qs(f)}`)
export const updateUser = (
  id: string,
  patch: { role?: string; status?: string; status_reason?: string; full_name?: string; password?: string },
) => platformFetch<Profile>(`/admin/users/${id}`, { method: 'PATCH', body: patch })

export const listInstitutions = (type?: InstitutionType) => platformFetch<Institution[]>(`/institutions${qs({ type })}`)
export const getStructure = (id: string) => platformFetch<Structure>(`/institutions/${id}/structure`)
export const createInstitution = (b: { type: InstitutionType; name_ar: string; name_en?: string; city?: string }) =>
  platformFetch<Institution>('/admin/institutions', { body: b })
export const updateInstitution = (id: string, b: Partial<Pick<Institution, 'name_ar' | 'name_en' | 'city' | 'is_active'>>) =>
  platformFetch<Institution>(`/admin/institutions/${id}`, { method: 'PATCH', body: b })
export const deleteInstitution = (id: string) => platformFetch<void>(`/admin/institutions/${id}`, { method: 'DELETE' })

export const createUnit = (b: { institution_id: string; parent_id?: string; kind: UnitKind; name_ar: string; name_en?: string }) =>
  platformFetch<Unit>('/admin/units', { body: b })
export const updateUnit = (id: string, b: Partial<Pick<Unit, 'name_ar' | 'name_en' | 'sort_order' | 'is_active'>>) =>
  platformFetch<Unit>(`/admin/units/${id}`, { method: 'PATCH', body: b })
export const deleteUnit = (id: string) => platformFetch<void>(`/admin/units/${id}`, { method: 'DELETE' })

export const createSubject = (b: { institution_id: string; unit_id?: string; name_ar: string; name_en?: string }) =>
  platformFetch<Subject>('/admin/subjects', { body: b })
export const updateSubject = (id: string, b: Partial<Pick<Subject, 'name_ar' | 'name_en' | 'is_active'>>) =>
  platformFetch<Subject>(`/admin/subjects/${id}`, { method: 'PATCH', body: b })
export const deleteSubject = (id: string) => platformFetch<void>(`/admin/subjects/${id}`, { method: 'DELETE' })
