<script setup lang="ts">
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt } from '@/i18n/platform'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()

async function logout() {
  await auth.signOut()
  router.replace('/mine')
}
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('platformHome') }}</h1>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('welcome') }} {{ auth.profile?.full_name }}
    </p>
    <div class="card-filled p-5 mb-4">
      <p class="text-body-lg">{{ pt('platformHomeSoon') }}</p>
    </div>
    <button class="btn-outlined" @click="logout">{{ pt('logout') }}</button>
  </div>
</template>
