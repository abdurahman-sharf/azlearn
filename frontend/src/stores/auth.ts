import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { PlatformError, getToken, platformEnabled, platformFetch, setToken } from '@/lib/platformApi'

export type PlatformRole = 'admin' | 'institution_admin' | 'moderator' | 'teacher' | 'student'
export type AccountStatus = 'active' | 'pending' | 'rejected' | 'suspended'
export type InstitutionType = 'school' | 'institute' | 'university'

export interface Profile {
  id: string
  email: string
  full_name: string
  role: PlatformRole
  institution_type: InstitutionType | null
  status: AccountStatus
  status_reason: string | null
}

/**
 * Called once the profile could finally be read after the network failed at start-up (registered by the router, which
 * then runs its guards again for the page the person is on). Module-level so this store never imports the router.
 */
let onRecovered: (() => void) | null = null
export function setRecoveredHandler(fn: (() => void) | null) {
  onRecovered = fn
}

export const useAuthStore = defineStore('auth', () => {
  const profile = ref<Profile | null>(null)
  const ready = ref(false)
  let initPromise: Promise<void> | null = null
  // true when the stored session turned out to be dead while the app was loading (the 401 handler does not run for the first navigation)
  let sessionLost = false
  /**
   * A session is stored but `/me` could not be answered (no network, server down): the session is NOT dropped, the app
   * shows an offline banner and tries again when the connection comes back or the tab returns to the front.
   */
  const offline = ref(false)
  /** Bumped when the profile arrives after such a failure: the shell remounts the page so it loads its data again. */
  const epoch = ref(0)
  let listening = false

  const isLoggedIn = computed(() => !!profile.value)
  const role = computed(() => profile.value?.role ?? null)
  const isActive = computed(() => profile.value?.status === 'active')

  function onOnline() {
    void retry()
  }
  function onVisible() {
    if (document.visibilityState === 'visible') void retry()
  }
  function listen(on: boolean) {
    if (typeof window === 'undefined' || on === listening) return
    listening = on
    if (on) {
      window.addEventListener('online', onOnline)
      document.addEventListener('visibilitychange', onVisible)
    } else {
      window.removeEventListener('online', onOnline)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }

  /**
   * Idempotent once it succeeded; the router guard awaits this before checking roles. A failure that is not a 401 (no
   * network, a server error) is never cached: the next call - or the retry on `online` / tab visible - asks again.
   */
  function init(): Promise<void> {
    if (initPromise) return initPromise
    const p: Promise<void> = (async () => {
      if (platformEnabled && getToken()) {
        try {
          profile.value = await platformFetch<Profile>('/me')
          offline.value = false
        } catch (e) {
          if (e instanceof PlatformError && e.status === 401) {
            // Only an explicit 401 means the session is gone.
            setToken(null)
            sessionLost = true
            offline.value = false
          } else {
            // Keep the token: the session is probably fine, the connection is not.
            offline.value = true
          }
        }
      } else {
        offline.value = false
      }
      listen(offline.value)
      ready.value = true
    })()
    initPromise = p
    void p.then(() => {
      if (offline.value && initPromise === p) initPromise = null
    })
    return p
  }

  /** Tries again to read the profile after a network failure (the banner's button, `online`, the tab coming back). */
  async function retry(): Promise<void> {
    if (!offline.value) return
    await init()
    if (!offline.value) {
      if (profile.value) epoch.value++
      onRecovered?.()
    }
  }

  /**
   * Forces a fresh `/me` (the cached profile can be stale: an admin may have approved or suspended the account since).
   * Errors are thrown to the caller; a 401 has already ended the session through the global handler.
   */
  async function refresh(): Promise<Profile | null> {
    if (!platformEnabled || !getToken()) return profile.value
    profile.value = await platformFetch<Profile>('/me')
    return profile.value
  }

  /** True once if the session stored in this browser was found dead while loading; lets the sign-in page say why. */
  function consumeSessionLost(): boolean {
    const was = sessionLost
    sessionLost = false
    return was
  }

  /** Drops the local session without calling the server (it is already gone there). */
  function clearSession() {
    profile.value = null
    offline.value = false
    listen(false)
  }

  async function signIn(email: string, password: string) {
    const res = await platformFetch<{ token: string; user: Profile }>('/login', { body: { email, password } })
    setToken(res.token)
    profile.value = res.user
    offline.value = false
    listen(false)
    initPromise = Promise.resolve()
  }

  /** Returns the new account status: 'active' (student) or 'pending' (teacher awaiting approval). */
  async function signUp(input: {
    email: string
    password: string
    fullName: string
    role: 'student' | 'teacher'
    institutionType: InstitutionType
    consent: boolean
  }): Promise<AccountStatus> {
    const res = await platformFetch<{ status: AccountStatus }>('/register', {
      body: {
        email: input.email,
        password: input.password,
        full_name: input.fullName,
        role: input.role,
        institution_type: input.institutionType,
        consent: input.consent,
      },
    })
    return res.status
  }

  async function signOut() {
    try {
      await platformFetch('/logout', { method: 'POST', body: {} })
    } catch {
      /* best effort: the local session is cleared regardless */
    }
    setToken(null)
    profile.value = null
    offline.value = false
    listen(false)
  }

  return {
    profile, ready, offline, epoch, isLoggedIn, role, isActive, init, retry, consumeSessionLost, refresh, clearSession, signIn, signUp, signOut,
    platformEnabled,
  }
})
