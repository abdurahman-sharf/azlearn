import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'

export interface BackupInfo {
  id: string
  kind: 'manual' | 'nightly'
  file: string
  created_at: number
  size: number
  files_count: number
  ok: boolean
  error: string | null
  /** False when the file is no longer in the server's backup folder. */
  present: boolean
  created_by: string | null
}

export type SystemWarning = 'no_backup' | 'backup_overdue' | 'last_backup_failed' | 'low_disk'

export interface SystemInfo {
  version: string
  uptime_sec: number
  db_bytes: number
  files_bytes: number
  files_count: number
  backups_bytes: number
  backups_count: number
  disk_free_bytes: number | null
  disk_total_bytes: number | null
  sessions_active: number
  users: number
  nightly_hour: number | null
  keep_nightly: number
  keep_manual: number
  backup_dir: string
  last_backup: BackupInfo | null
  last_failure: BackupInfo | null
  warnings: SystemWarning[]
}

export const systemInfo = () => platformFetch<SystemInfo>('/admin/system')
export const listBackups = () => platformFetch<BackupInfo[]>('/admin/backups')
export const createBackup = () => platformFetch<BackupInfo>('/admin/backup', { method: 'POST' })

/** Downloads through fetch so the Bearer token is sent, then saves the blob under the server's file name. */
export async function downloadBackup(b: Pick<BackupInfo, 'id' | 'file'>): Promise<void> {
  const token = getToken()
  const res = await fetch(`/api/platform/admin/backups/${encodeURIComponent(b.id)}`, { headers: token ? { Authorization: `Bearer ${token}` } : {} }).catch(() => {
    throw new PlatformError('network', 0)
  })
  if (!res.ok) throw new PlatformError(((await res.json().catch(() => ({}))) as { error?: string }).error ?? 'unknown', res.status)
  const m = /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')
  const url = URL.createObjectURL(await res.blob())
  const a = document.createElement('a')
  a.href = url
  a.download = m?.[1] ?? b.file
  a.style.display = 'none'
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

/** `12.3 MB` — binary units, one decimal from KB up. */
export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`
}

/** `3d 4h 12m` — the largest two non-zero parts. */
export function fmtDuration(sec: number): string {
  const d = Math.floor(sec / 86400)
  const h = Math.floor((sec % 86400) / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const parts = [d && `${d}d`, (d || h) && `${h}h`, `${m}m`].filter(Boolean) as string[]
  return parts.slice(0, 2).join(' ')
}
