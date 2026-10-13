import { ref } from 'vue'
import { adminStats, type Stats } from '@/api/platformOps'

// One shared copy of the dashboard numbers: the admin sidebar badges and the overview page read the
// same request, so navigating inside the admin area never fetches stats twice for one screen.
const stats = ref<Stats | null>(null)
let inflight: Promise<void> | null = null

/** Forgets the numbers (called on sign-out so the next account never sees a stale badge). */
export function resetAdminStats() {
  stats.value = null
}

export function useAdminStats() {
  const refresh = () => {
    inflight ??= adminStats()
      .then((s) => { stats.value = s })
      .catch(() => { /* badges are optional */ })
      .finally(() => { inflight = null })
    return inflight
  }
  return { stats, refresh }
}
