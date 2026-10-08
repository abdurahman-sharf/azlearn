import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { resetAdminStats } from '@/lib/adminStats'
import { resetTeacherStats } from '@/lib/teacherStats'
import { askClearLocalBanks } from '@/composables/signOutBank'
import { hasLocalQuestions, LOCAL_QUESTION_CLEAR_KEYS } from '@/utils/localBanks'

/** True when this browser holds question banks / generated questions (storage may be blocked: then there is nothing to offer). */
function storedQuestions(): boolean {
  try {
    return hasLocalQuestions(localStorage)
  } catch {
    return false
  }
}

/** Removes the stored banks and generated questions, and the copies the stores keep in memory (sign-out does not reload the page). */
async function clearLocalQuestions() {
  try {
    for (const key of LOCAL_QUESTION_CLEAR_KEYS) localStorage.removeItem(key)
  } catch { /* storage unavailable */ }
  // imported lazily: these stores are big and only needed on this rare path
  const [{ usePracticeStore }, { useExamStore }] = await Promise.all([import('@/stores/practice'), import('@/stores/exam')])
  const practice = usePracticeStore()
  practice.banks = []
  practice.clearSession()
  useExamStore().reset()
}

/**
 * Signs out and sends the user to the landing page. The session must be gone before navigating: the router's `home`
 * guard sends signed-in users to `/platform`. `useRouter()` (instead of importing the router) avoids the
 * router ⇄ auth-store import cycle.
 *
 * When the browser holds local question banks the person is asked whether to wipe them from this device (default: keep;
 * the question never appears when nothing is stored). The banks are not tied to the account, so on a shared computer
 * the next visitor could otherwise read them.
 */
export function useSignOut() {
  const auth = useAuthStore()
  const router = useRouter()
  return async () => {
    const clear = storedQuestions() ? await askClearLocalBanks() : false
    await auth.signOut()
    resetAdminStats()
    resetTeacherStats()
    try { localStorage.removeItem('exameow-admin-subject') } catch { /* storage unavailable */ }
    if (clear) await clearLocalQuestions().catch(() => { /* the keys are already gone; a store that cannot be reset is not worth blocking sign-out */ })
    await router.replace('/')
  }
}
