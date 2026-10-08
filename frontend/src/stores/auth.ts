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

export const useAuthStore = defineStore('auth', () => {
  const profile = ref<Profile | null>(null)
  const ready = ref(false)
  let initPromise: Promise<void> | null = null
  // true when the stored session turned out to be dead while the app was loading (the 401 handler does not run for the first navigation)
  let sessionLost = false

  const isLoggedIn = computed(() => !!profile.value)
  const role = computed(() => profile.value?.role ?? null)
  const isActive = computed(() => profile.value?.status === 'active')

  /** Idempotent; the router guard awaits this before checking roles. */
  function init(): Promise<void> {
    if (initPromise) return initPromise
    initPromise = (async () => {
      if (platformEnabled && getToken()) {
        try {
          profile.value = await platformFetch<Profile>('/me')
        } catch (e) {
          // Only an explicit 401 means the session is gone; keep the token on network errors.
          if (e instanceof PlatformError && e.status === 401) {
            setToken(null)
            sessionLost = true
          }
        }
      }
      ready.value = true
    })()
    return initPromise
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
  }

  async function signIn(email: string, password: string) {
    const res = await platformFetch<{ token: string; user: Profile }>('/login', { body: { email, password } })
    setToken(res.token)
    profile.value = res.user
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
  }

  return { profile, ready, isLoggedIn, role, isActive, init, consumeSessionLost, refresh, clearSession, signIn, signUp, signOut, platformEnabled }
})
