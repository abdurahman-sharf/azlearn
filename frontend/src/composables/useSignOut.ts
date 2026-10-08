import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { resetAdminStats } from '@/lib/adminStats'
import { resetTeacherStats } from '@/lib/teacherStats'

/**
 * Signs out and sends the user to the landing page. The session must be gone before navigating: the router's `home`
 * guard sends signed-in users to `/platform`. `useRouter()` (instead of importing the router) avoids the
 * router ⇄ auth-store import cycle.
 */
export function useSignOut() {
  const auth = useAuthStore()
  const router = useRouter()
  return async () => {
    await auth.signOut()
    resetAdminStats()
    resetTeacherStats()
    try { localStorage.removeItem('exameow-admin-subject') } catch { /* storage unavailable */ }
    await router.replace('/')
  }
}
