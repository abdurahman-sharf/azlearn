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
          if (e instanceof PlatformError && e.status === 401) setToken(null)
        }
      }
      ready.value = true
    })()
    return initPromise
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

  return { profile, ready, isLoggedIn, role, isActive, init, signIn, signUp, signOut, platformEnabled }
})
