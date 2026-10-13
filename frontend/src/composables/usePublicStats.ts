import { ref } from 'vue'
import { platformEnabled } from '@/lib/platformApi'
import { publicStats, type PublicStats } from '@/api/platformStats'

const CACHE_KEY = 'exameow-public-stats'
const stats = ref<PublicStats | null>(readCache())
let inflight: Promise<void> | null = null

function readCache(): PublicStats | null {
  try {
    const c = JSON.parse(localStorage.getItem(CACHE_KEY) ?? 'null') as Partial<PublicStats> | null
    return c && [c.questions, c.subjects, c.attempts].every((n) => Number.isFinite(n)) ? (c as PublicStats) : null
  } catch {
    return null
  }
}

/**
 * The landing page counters: the last known numbers show at once (cache), then the server's replace them. A failed
 * request keeps what is there, so the page never shows an error for a decorative band.
 */
export function usePublicStats() {
  const load = () => {
    if (!platformEnabled) return Promise.resolve()
    inflight ??= publicStats()
      .then((s) => {
        stats.value = s
        try { localStorage.setItem(CACHE_KEY, JSON.stringify(s)) } catch { /* storage unavailable */ }
      })
      .catch(() => { /* offline or server error */ })
      .finally(() => { inflight = null })
    return inflight
  }
  return { stats, load }
}
