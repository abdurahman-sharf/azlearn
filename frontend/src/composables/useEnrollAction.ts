import { ref } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { enroll } from '@/api/platformLearning'
import { useStudentStats } from '@/lib/studentStats'

/**
 * "Enrol in this subject" from a page that shows something only enrolled students may see (a lesson video, a live link):
 * one request at a time, the error text for the page, and the student stats (sidebar badges, overview) refreshed after the
 * change. The page passes what to do once the enrolment is in (reload its own data).
 */
export function useEnrollAction() {
  const pt = usePt()
  const { refreshAfterChange } = useStudentStats()
  const busy = ref(false)
  const error = ref('')

  async function run(subjectId: string, after: () => Promise<unknown> | unknown): Promise<boolean> {
    if (busy.value) return false
    busy.value = true
    error.value = ''
    try {
      await enroll(subjectId)
      void refreshAfterChange()
      await after()
      return true
    } catch (e) {
      error.value = platformErrorMessage(pt, e)
      return false
    } finally {
      busy.value = false
    }
  }
  return { busy, error, run }
}
