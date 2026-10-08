<script setup lang="ts">
import { ref } from 'vue'
import { useAuthStore, type InstitutionType } from '@/stores/auth'
import { usePt, platformErrorKey } from '@/i18n/platform'
import { onMounted } from 'vue'
import { useBrandingStore } from '@/stores/branding'
import BrandMark from '@/components/platform/BrandMark.vue'
import LegalLinks from '@/components/platform/LegalLinks.vue'
import { PlatformError } from '@/lib/platformApi'

const pt = usePt()
const auth = useAuthStore()
const brand = useBrandingStore()
onMounted(() => brand.load())
const consent = ref(false)

const fullName = ref('')
const email = ref('')
const password = ref('')
const role = ref<'student' | 'teacher'>('student')
const institutionType = ref<InstitutionType>('school')
const error = ref('')
const done = ref<'active' | 'pending' | null>(null)
const loading = ref(false)

const types: { value: InstitutionType; key: 'typeSchool' | 'typeInstitute' | 'typeUniversity' }[] = [
  { value: 'school', key: 'typeSchool' },
  { value: 'institute', key: 'typeInstitute' },
  { value: 'university', key: 'typeUniversity' },
]

async function submit() {
  error.value = ''
  loading.value = true
  try {
    const status = await auth.signUp({
      email: email.value.trim(),
      password: password.value,
      fullName: fullName.value.trim(),
      role: role.value,
      institutionType: institutionType.value,
      consent: consent.value,
    })
    done.value = status === 'pending' ? 'pending' : 'active'
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
    <h1 class="text-display-sm font-bold tracking-tight mb-6">{{ pt('register') }}</h1>

    <div v-if="done" class="card-elevated p-6 space-y-4">
      <p class="text-body-lg">{{ done === 'pending' ? pt('registeredTeacher') : pt('registered') }}</p>
      <router-link to="/auth/login" class="btn-filled inline-flex">{{ pt('login') }}</router-link>
    </div>

    <form v-else class="card-elevated p-6 space-y-4" @submit.prevent="submit">
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('fullName') }}</span>
        <input v-model="fullName" required maxlength="120" autocomplete="name" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('email') }}</span>
        <input v-model="email" type="email" required autocomplete="email" dir="ltr" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('password') }}</span>
        <input v-model="password" type="password" required minlength="8" autocomplete="new-password" dir="ltr" class="input-outlined mt-1 w-full" />
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('passwordHint') }}</span>
      </label>

      <fieldset>
        <legend class="text-label-lg font-semibold mb-2">{{ pt('accountRole') }}</legend>
        <div class="flex gap-2">
          <button type="button" :class="role === 'student' ? 'btn-filled' : 'btn-outlined'" class="flex-1" @click="role = 'student'">{{ pt('roleStudent') }}</button>
          <button type="button" :class="role === 'teacher' ? 'btn-filled' : 'btn-outlined'" class="flex-1" @click="role = 'teacher'">{{ pt('roleTeacher') }}</button>
        </div>
        <p v-if="role === 'teacher'" class="text-body-sm mt-2" style="color: rgb(var(--md-on-surface-variant))">{{ pt('teacherNotice') }}</p>
      </fieldset>

      <fieldset>
        <legend class="text-label-lg font-semibold mb-2">{{ pt('institutionType') }}</legend>
        <div class="flex gap-2">
          <button
            v-for="t in types"
            :key="t.value"
            type="button"
            :class="institutionType === t.value ? 'btn-tonal' : 'btn-outlined'"
            class="flex-1"
            @click="institutionType = t.value"
          >{{ pt(t.key) }}</button>
        </div>
      </fieldset>

      <label class="flex items-start gap-2 text-body-sm" data-testid="consent">
        <input v-model="consent" type="checkbox" required class="mt-1" data-testid="consent-box" />
        <span>
          {{ pt('consentLabel') }}
          <router-link v-if="brand.legal.terms" to="/legal/terms" target="_blank" class="underline">{{ pt('termsPage') }}</router-link>
          <template v-else>{{ pt('termsPage') }}</template>
          {{ pt('consentAnd') }}
          <router-link v-if="brand.legal.privacy" to="/legal/privacy" target="_blank" class="underline">{{ pt('privacyPage') }}</router-link>
          <template v-else>{{ pt('privacyPage') }}</template>
        </span>
      </label>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <button type="submit" class="btn-filled w-full" :disabled="loading || !consent" data-testid="register-submit">{{ pt('register') }}</button>
      <p class="text-body-sm text-center" style="color: rgb(var(--md-on-surface-variant))">
        {{ pt('haveAccount') }}
        <router-link to="/auth/login" class="font-semibold underline">{{ pt('login') }}</router-link>
      </p>
    </form>
    <div class="mt-5"><LegalLinks /></div>
  </div>
</template>
