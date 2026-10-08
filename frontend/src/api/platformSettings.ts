import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'

export interface AiSettings {
  endpoint: string
  model: string
  key_saved: boolean
  source: 'db' | 'env' | 'none'
  effective_endpoint: string
  effective_model: string
}
export interface Settings {
  ai: AiSettings
  cap_platform: number
  cap_admin: number
  teachers_can_create_exams: boolean
}
export interface Usage {
  today_platform: number
  today_mine: number
  cap_platform: number
  cap_admin: number
  resets_at: number
  days: { day: number; calls: number }[]
  admins_today: { user_id: string; name: string; calls: number }[]
}
export interface TestResult { ok: boolean; ms: number; code: string; detail: string }

export const getSettings = () => platformFetch<Settings>('/admin/settings')
/** `api_key: ''` deletes the saved key; omit a field to leave it untouched. */
export const saveSettings = (b: Partial<{ endpoint: string; model: string; api_key: string; cap_platform: number; cap_admin: number; teachers_can_create_exams: boolean }>) =>
  platformFetch<Settings>('/admin/settings', { method: 'PUT', body: b })
export const testAi = (b: { endpoint?: string; model?: string; api_key?: string }) =>
  platformFetch<TestResult>('/admin/settings/ai/test', { method: 'POST', body: b })
export const aiUsage = () => platformFetch<Usage>('/admin/ai/usage')

export const saveBranding = (b: { name?: string; color?: string }) => platformFetch<void>('/admin/branding', { method: 'PUT', body: b })
export const deleteLogo = () => platformFetch<void>('/admin/branding/logo', { method: 'DELETE' })
export async function uploadLogo(file: File): Promise<void> {
  const fd = new FormData()
  fd.append('file', file)
  const token = getToken()
  const res = await fetch('/api/platform/admin/branding/logo', {
    method: 'POST',
    headers: token ? { Authorization: `Bearer ${token}` } : {},
    body: fd,
  }).catch(() => { throw new PlatformError('network', 0) })
  if (!res.ok) {
    const data = await res.json().catch(() => ({}))
    throw new PlatformError((data as { error?: string }).error ?? (res.status === 413 ? 'invalid_file_size' : 'unknown'), res.status)
  }
}
export const getLegal = (slug: 'privacy' | 'terms') =>
  platformFetch<{ body: string | null }>(`/public/legal/${slug}`).then((r) => r.body ?? '')
export const saveLegal = (slug: 'privacy' | 'terms', body: string) => platformFetch<void>(`/admin/legal/${slug}`, { method: 'PUT', body: { body } })
