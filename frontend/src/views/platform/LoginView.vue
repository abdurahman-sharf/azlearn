<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorKey } from '@/i18n/platform'
import { onMounted } from 'vue'
import { useBrandingStore } from '@/stores/branding'
import BrandMark from '@/components/platform/BrandMark.vue'
import LegalLinks from '@/components/platform/LegalLinks.vue'
import { PlatformError } from '@/lib/platformApi'
import { isExamTakePath } from '@/utils/platformGuard'

const pt = usePt()
const auth = useAuthStore()
onMounted(() => useBrandingStore().load())
const router = useRouter()
const route = useRoute()

// The session ended while the person was on a page (see the router's unauthorized handler): say so instead of
// silently turning the screen into a login form. An exam in progress keeps its answers in a local draft.
const expired = computed(() => route.query.expired === '1')
const examKept = computed(() => expired.value && isExamTakePath(route.query.redirect))

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
    error.value = pt(platformErrorKey(e instanceof PlatformError ? e.code : ''))
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <div class="flex justify-center mb-4"><router-link to="/"><BrandMark :size="40" /></router-link></div>
    <h1 class="text-display-sm font-bold tracking-tight mb-6">{{ pt('login') }}</h1>
    <p v-if="expired" class="card-filled p-3 text-body-sm mb-4" role="status" data-testid="login-expired">
      {{ pt('unauthorized') }}<template v-if="examKept"> {{ pt('sessionExamKept') }}</template>
    </p>
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
    <div class="mt-5"><LegalLinks /></div>
  </div>
</template>
