<script setup lang="ts">
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt } from '@/i18n/platform'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()
const route = useRoute()

const email = ref('')
const password = ref('')
const error = ref('')
const loading = ref(false)

async function submit() {
  error.value = ''
  loading.value = true
  try {
    await auth.signIn(email.value.trim(), password.value)
    const redirect = typeof route.query.redirect === 'string' && route.query.redirect.startsWith('/')
      ? route.query.redirect
      : '/platform'
    router.replace(redirect)
  } catch (e: any) {
    error.value = e?.message === 'platform-off' ? pt('platformOff') : pt('invalidCredentials')
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight mb-6">{{ pt('login') }}</h1>
    <form class="card-elevated p-6 space-y-4" @submit.prevent="submit">
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('email') }}</span>
        <input v-model="email" type="email" required autocomplete="email" dir="ltr" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('password') }}</span>
        <input v-model="password" type="password" required autocomplete="current-password" dir="ltr" class="input-outlined mt-1 w-full" />
      </label>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <button type="submit" class="btn-filled w-full" :disabled="loading">{{ pt('login') }}</button>
      <p class="text-body-sm text-center" style="color: rgb(var(--md-on-surface-variant))">
        {{ pt('noAccount') }}
        <router-link to="/auth/register" class="font-semibold underline">{{ pt('register') }}</router-link>
      </p>
    </form>
  </div>
</template>
