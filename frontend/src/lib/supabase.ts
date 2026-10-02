import { createClient, type SupabaseClient } from '@supabase/supabase-js'

const url = import.meta.env.VITE_SUPABASE_URL as string | undefined
const anonKey = import.meta.env.VITE_SUPABASE_ANON_KEY as string | undefined

/** The platform (accounts / roles) is optional: without env vars the app behaves as before. */
export const platformEnabled = Boolean(url && anonKey)

export const supabase: SupabaseClient | null = platformEnabled
  ? createClient(url!, anonKey!, { auth: { persistSession: true, autoRefreshToken: true } })
  : null
