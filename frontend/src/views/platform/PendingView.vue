<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useSignOut } from '@/composables/useSignOut'

// Where an account waits for (or learns the outcome of) the admin's decision. The cached profile can be stale, so the
// page re-reads /me: on demand, every POLL_MS while the tab is visible, and when the tab comes back to the front.
// As soon as the account is active it moves on by itself.
const POLL_MS = 45_000
const pt = usePt()
const auth = useAuthStore()
const router = useRouter()
const logout = useSignOut()

const checking = ref(false)
const checked = ref(false)
const error = ref('')
let timer: ReturnType<typeof setInterval> | null = null

const status = computed(() => auth.profile?.status)
const title = computed(() => {
  if (status.value === 'rejected') return pt('rejectedTitle')
  if (status.value === 'suspended') return pt('suspendedTitle')
  return pt('pendingTitle')
})
const reason = computed(() => auth.profile?.status_reason?.trim() || '')

async function check(manual: boolean) {
  if (checking.value) return
  checking.value = true
  if (manual) { error.value = ''; checked.value = false }
  try {
    await auth.refresh()
    if (manual) checked.value = true
  } catch (e) {
    if (manual) error.value = platformErrorMessage(pt, e) // a failed background check stays silent
  } finally {
    checking.value = false
  }
}

// Approved: leave the waiting room (the router guard does the same on a reload).
watch(() => auth.isActive, (active) => { if (active) router.replace('/platform') }, { immediate: true })

const visible = () => document.visibilityState === 'visible'
function onVisibility() {
  if (visible()) check(false)
}
onMounted(() => {
  timer = setInterval(() => { if (visible()) check(false) }, POLL_MS)
  document.addEventListener('visibilitychange', onVisibility)
})
onBeforeUnmount(() => {
  if (timer) clearInterval(timer)
  document.removeEventListener('visibilitychange', onVisibility)
})
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <div class="card-elevated p-6 space-y-4" data-testid="pending-card">
      <h1 class="text-headline-sm font-bold" data-testid="pending-title">{{ title }}</h1>

      <template v-if="status === 'pending'">
        <p class="text-body-lg">{{ pt('pendingDesc') }}</p>
        <section aria-labelledby="pending-next-title" data-testid="pending-next">
          <h2 id="pending-next-title" class="text-title-md font-bold mb-2">{{ pt('pendingNextTitle') }}</h2>
          <ol class="list-decimal ps-5 space-y-1 text-body-md">
            <li>{{ pt('pendingNext1') }}</li>
            <li>{{ pt('pendingNext2') }}</li>
            <li>{{ pt('pendingNext3') }}</li>
          </ol>
        </section>
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('pendingAuto') }}</p>
        <!-- a teacher can already queue the subjects they teach; the admin sees them next to the account decision -->
        <router-link v-if="auth.role === 'teacher'" to="/platform/teaching" class="btn-tonal inline-flex" data-testid="pending-request-subjects">{{ pt('pendingRequestSubjects') }}</router-link>
      </template>
      <template v-else>
        <p v-if="reason" class="text-body-lg" data-testid="pending-reason">{{ pt('reason') }}: <span dir="auto">{{ reason }}</span></p>
        <p v-else class="text-body-lg" data-testid="pending-no-reason">{{ pt('pendingNoReason') }}</p>
        <p class="text-body-md">{{ pt('pendingContact') }}</p>
      </template>

      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="pending-error">{{ error }}</p>
      <p v-else-if="checked && status === 'pending'" class="text-body-sm" role="status" data-testid="pending-still">{{ pt('pendingStillWaiting') }}</p>

      <div class="flex flex-wrap gap-2">
        <button type="button" class="btn-filled" :disabled="checking" data-testid="pending-refresh" @click="check(true)">{{ checking ? pt('pendingChecking') : pt('pendingCheck') }}</button>
        <button type="button" class="btn-outlined" data-testid="pending-logout" @click="logout">{{ pt('logout') }}</button>
      </div>
    </div>
  </div>
</template>
