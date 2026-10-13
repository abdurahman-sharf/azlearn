import { ref } from 'vue'
import { studentOverview, type StudentOverview } from '@/api/platformStudent'

// One shared copy of the student's overview: the sidebar badges (exams to act on, unread notifications) and the overview
// page read the same request, so moving around the student area never fetches it twice for one screen.
const stats = ref<StudentOverview | null>(null)
const failed = ref(false)
let inflight: Promise<void> | null = null
// Bumped on sign-out / a lost session: an answer that arrives after it must not leak into the next account's badges.
let generation = 0

/** Forgets the numbers (called on sign-out and on a lost session so the next account never sees a stale badge). */
export function resetStudentStats() {
  generation++
  stats.value = null
  failed.value = false
  inflight = null
}

export function useStudentStats() {
  const refresh = () => {
    if (!inflight) {
      const mine = generation
      const p: Promise<void> = studentOverview()
        .then((s) => {
          if (mine !== generation) return
          stats.value = s
          failed.value = false
        })
        .catch(() => {
          if (mine === generation) failed.value = true // badges are optional; the overview page shows the error
        })
        .finally(() => {
          if (inflight === p) inflight = null
        })
      inflight = p
    }
    return inflight
  }
  /**
   * For use right after the student changed something the numbers depend on (read the notifications, enrolled in a subject):
   * a request already in flight may have been sent before the change, so wait for it and ask again.
   */
  const refreshAfterChange = async () => {
    if (inflight) await inflight
    return refresh()
  }
  return { stats, failed, refresh, refreshAfterChange }
}
