<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt } from '@/i18n/platform'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()

const title = computed(() => {
  const s = auth.profile?.status
  if (s === 'rejected') return pt('rejectedTitle')
  if (s === 'suspended') return pt('suspendedTitle')
  return pt('pendingTitle')
})

async function logout() {
  await auth.signOut()
  router.replace('/auth/login')
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <div class="card-elevated p-6 space-y-4">
      <h1 class="text-headline-sm font-bold">{{ title }}</h1>
      <p v-if="auth.profile?.status === 'pending'" class="text-body-lg">{{ pt('pendingDesc') }}</p>
      <p v-else-if="auth.profile?.status_reason" class="text-body-lg">{{ pt('reason') }}: {{ auth.profile.status_reason }}</p>
      <button class="btn-outlined" @click="logout">{{ pt('logout') }}</button>
    </div>
  </div>
</template>
