import { ref } from 'vue'

// State of the "clear the local question banks?" question shown at sign-out. It is a few module-level values (not a
// Pinia store) so `useSignOut` can ask from anywhere without importing a store that imports the auth store, and so the
// single dialog mounted in AppShell and every sign-out button share it.

/** True while the question is on screen. */
export const signOutBankOpen = ref(false)
let resolver: ((clear: boolean) => void) | null = null

/** Opens the question; resolves `true` when the person chose to clear the banks, `false` for keep (also for Escape). */
export function askClearLocalBanks(): Promise<boolean> {
  // a second ask while one is open settles the first as "keep" (never clear on a stale question)
  resolver?.(false)
  return new Promise<boolean>((resolve) => {
    resolver = resolve
    signOutBankOpen.value = true
  })
}

export function answerClearLocalBanks(clear: boolean) {
  const r = resolver
  resolver = null
  signOutBankOpen.value = false
  r?.(clear)
}
