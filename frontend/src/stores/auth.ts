import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import type { Session } from '@supabase/supabase-js'
import { supabase, platformEnabled } from '@/lib/supabase'

export type PlatformRole = 'admin' | 'institution_admin' | 'moderator' | 'teacher' | 'student'
export type AccountStatus = 'active' | 'pending' | 'rejected' | 'suspended'
export type InstitutionType = 'school' | 'institute' | 'university'

export interface Profile {
  id: string
  full_name: string
  role: PlatformRole
  institution_type: InstitutionType | null
  status: AccountStatus
  status_reason: string | null
}

export const useAuthStore = defineStore('auth', () => {
  const session = ref<Session | null>(null)
  const profile = ref<Profile | null>(null)
  const ready = ref(false)
  let initPromise: Promise<void> | null = null

  const isLoggedIn = computed(() => !!session.value)
  const role = computed(() => profile.value?.role ?? null)
  const isActive = computed(() => profile.value?.status === 'active')

  async function loadProfile() {
    if (!supabase || !session.value) {
      profile.value = null
      return
    }
    const { data } = await supabase
      .from('profiles')
      .select('id, full_name, role, institution_type, status, status_reason')
      .eq('id', session.value.user.id)
      .maybeSingle()
    profile.value = (data as Profile | null) ?? null
  }

  /** Idempotent; the router guard awaits this before checking roles. */
  function init(): Promise<void> {
    if (initPromise) return initPromise
    initPromise = (async () => {
      if (supabase) {
        const { data } = await supabase.auth.getSession()
        session.value = data.session
        await loadProfile()
        supabase.auth.onAuthStateChange((_event, s) => {
          session.value = s
          // Defer: calling supabase inside this callback can deadlock.
          setTimeout(() => void loadProfile(), 0)
        })
      }
      ready.value = true
    })()
    return initPromise
  }

  async function signIn(email: string, password: string) {
    if (!supabase) throw new Error('platform-off')
    const { error } = await supabase.auth.signInWithPassword({ email, password })
    if (error) throw error
    await loadProfile()
  }

  /** Returns true when an email confirmation is required before signing in. */
  async function signUp(input: {
    email: string
    password: string
    fullName: string
    role: 'student' | 'teacher'
    institutionType: InstitutionType
  }): Promise<boolean> {
    if (!supabase) throw new Error('platform-off')
    const { data, error } = await supabase.auth.signUp({
      email: input.email,
      password: input.password,
      options: {
        data: {
          full_name: input.fullName,
          role: input.role,
          institution_type: input.institutionType,
        },
      },
    })
    if (error) throw error
    return !data.session
  }

  async function signOut() {
    if (supabase) await supabase.auth.signOut()
    session.value = null
    profile.value = null
  }

  return { session, profile, ready, isLoggedIn, role, isActive, init, signIn, signUp, signOut, platformEnabled }
})
