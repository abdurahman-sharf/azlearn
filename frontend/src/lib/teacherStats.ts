import { ref } from 'vue'
import { teacherStats, type TeacherStats } from '@/api/platformTeacher'

// One shared copy of the teacher's numbers: the sidebar badges, the overview page and the pages that gate buttons on
// `can_create_exams` read the same request, so moving around the teacher area never fetches them twice for one screen.
const stats = ref<TeacherStats | null>(null)
const failed = ref(false)
let inflight: Promise<void> | null = null
// Bumped on sign-out: an answer that arrives after it must not leak into the next account's badges.
let generation = 0

/** Forgets the numbers (called on sign-out and on a lost session so the next account never sees a stale badge). */
export function resetTeacherStats() {
  generation++
  stats.value = null
  failed.value = false
  inflight = null
}

export function useTeacherStats() {
  const refresh = () => {
    if (!inflight) {
      const mine = generation
      const p: Promise<void> = teacherStats()
        .then((s) => {
          if (mine !== generation) return
          stats.value = s
          failed.value = false
        })
        .catch(() => {
          if (mine === generation) failed.value = true // badges are optional; the overview page shows a notice
        })
        .finally(() => {
          if (inflight === p) inflight = null
        })
      inflight = p
    }
    return inflight
  }
  return { stats, failed, refresh }
}
