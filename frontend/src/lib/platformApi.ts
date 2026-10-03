import { isCloudflare, isTauri } from '@/utils/platform'

const TOKEN_KEY = 'exameow-platform-token'

/**
 * Accounts live in the self-hosted Axum server (same origin for web/Docker).
 * Cloudflare and Tauri builds have no platform server unless VITE_PLATFORM_API points to one.
 */
const base = ((import.meta.env.VITE_PLATFORM_API as string | undefined) ?? '').replace(/\/$/, '')
export const platformEnabled = !!base || (!isCloudflare() && !isTauri())

export class PlatformError extends Error {
  constructor(public code: string, public status: number) {
    super(code)
  }
}

export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY)
  } catch {
    return null
  }
}

export function setToken(token: string | null) {
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token)
    else localStorage.removeItem(TOKEN_KEY)
  } catch {
    /* storage unavailable: session lasts until reload */
  }
}

export async function platformFetch<T>(path: string, init: { method?: string; body?: unknown } = {}): Promise<T> {
  const token = getToken()
  let res: Response
  try {
    res = await fetch(`${base}/api/platform${path}`, {
      method: init.method ?? (init.body ? 'POST' : 'GET'),
      headers: {
        ...(init.body ? { 'Content-Type': 'application/json' } : {}),
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      body: init.body ? JSON.stringify(init.body) : undefined,
    })
  } catch {
    throw new PlatformError('network', 0)
  }
  if (res.status === 204) return undefined as T
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new PlatformError((data as { error?: string }).error ?? 'unknown', res.status)
  return data as T
}
