<script setup lang="ts">
import { ref } from 'vue'
import { useAuthStore, type InstitutionType } from '@/stores/auth'
import { usePt } from '@/i18n/platform'

const pt = usePt()
const auth = useAuthStore()

const fullName = ref('')
const email = ref('')
const password = ref('')
const role = ref<'student' | 'teacher'>('student')
const institutionType = ref<InstitutionType>('school')
const error = ref('')
const done = ref(false)
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
    await auth.signUp({
      email: email.value.trim(),
      password: password.value,
      fullName: fullName.value.trim(),
      role: role.value,
      institutionType: institutionType.value,
    })
    done.value = true
  } catch (e: any) {
    error.value = e?.message === 'platform-off' ? pt('platformOff') : (e?.message || pt('genericError'))
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight mb-6">{{ pt('register') }}</h1>

    <div v-if="done" class="card-elevated p-6 space-y-4">
      <p class="text-body-lg">{{ pt('checkEmail') }}</p>
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

      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <button type="submit" class="btn-filled w-full" :disabled="loading">{{ pt('register') }}</button>
      <p class="text-body-sm text-center" style="color: rgb(var(--md-on-surface-variant))">
        {{ pt('haveAccount') }}
        <router-link to="/auth/login" class="font-semibold underline">{{ pt('login') }}</router-link>
      </p>
    </form>
  </div>
</template>
