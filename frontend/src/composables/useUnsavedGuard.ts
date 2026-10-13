import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'

/**
 * "You have unsaved changes" protection for an editor.
 *
 * `getState` returns everything the form holds (any JSON-able value); the guard compares it with the copy taken by the
 * last `markClean()` and, while they differ, (1) asks before the router leaves the page (the answer comes through
 * `asking` / `answer`, shown by <UnsavedChangesDialog>) and (2) asks the browser to confirm closing or reloading the tab.
 *
 * Call `markClean()` once the form has been filled from the server and again after every successful save - BEFORE
 * navigating away (a create navigates to the new item's edit page). Until the first `markClean()` nothing is dirty, so a
 * form that is still loading never blocks the way out. A person whose session has ended (sign-out, expired token) is
 * never asked: there is nothing left to save to and the redirect to the sign-in page must not be held up.
 */
export function useUnsavedGuard(getState: () => unknown) {
  const auth = useAuthStore()
  const router = useRouter()
  const baseline = ref<string | null>(null)
  const asking = ref(false)
  let pending: ((leave: boolean) => void) | null = null

  const serialize = () => {
    try {
      return JSON.stringify(getState())
    } catch {
      return ''
    }
  }
  const dirty = computed(() => baseline.value !== null && serialize() !== baseline.value)

  /** Takes the current form state as the saved one. */
  function markClean() {
    baseline.value = serialize()
  }

  /** Settles the open question: `true` = leave and drop the changes, `false` = stay on the page. */
  function answer(leave: boolean) {
    asking.value = false
    const p = pending
    pending = null
    p?.(leave)
  }

  // A global guard (not onBeforeRouteLeave): a create navigates from the "new" route to the item's "edit" route while the
  // same component instance stays on screen, and a per-route guard would stay attached to the old route record.
  const removeGuard = router.beforeEach((to, from) => {
    if (to.fullPath === from.fullPath || !dirty.value || !auth.isLoggedIn) return true
    // a second navigation while a question is open cancels the first one
    pending?.(false)
    asking.value = true
    return new Promise<boolean>((resolve) => {
      pending = resolve
    })
  })

  const onBeforeUnload = (e: BeforeUnloadEvent) => {
    if (!dirty.value || !auth.isLoggedIn) return
    e.preventDefault()
    e.returnValue = '' // some browsers only show their own prompt when this is set
  }
  onMounted(() => window.addEventListener('beforeunload', onBeforeUnload))
  onBeforeUnmount(() => {
    removeGuard()
    window.removeEventListener('beforeunload', onBeforeUnload)
    // an unanswered question must not leave the router waiting forever
    if (pending) answer(false)
  })

  return { dirty, markClean, asking, answer }
}
