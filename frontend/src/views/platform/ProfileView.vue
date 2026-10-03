<script setup lang="ts">
import { ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { updateProfile, getTeacher } from '@/api/platformLearning'
import { onMounted } from 'vue'

const pt = usePt()
const auth = useAuthStore()
const fullName = ref(auth.profile?.full_name ?? '')
const bio = ref('')
const msg = ref('')
const error = ref('')

onMounted(async () => {
  if (auth.role === 'teacher' && auth.profile) {
    try { bio.value = (await getTeacher(auth.profile.id)).bio ?? '' } catch { /* bio is optional */ }
  }
})

async function save() {
  msg.value = error.value = ''
  try {
    await updateProfile({ full_name: fullName.value, ...(auth.role === 'teacher' ? { bio: bio.value } : {}) })
    if (auth.profile) auth.profile.full_name = fullName.value.trim()
    msg.value = pt('saved')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myProfile') }}</h1>
    <form class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('fullName') }}</span>
        <input v-model="fullName" required maxlength="120" class="input-outlined mt-1 w-full" />
      </label>
      <label v-if="auth.role === 'teacher'" class="block">
        <span class="text-label-lg font-semibold">{{ pt('bio') }}</span>
        <textarea v-model="bio" maxlength="1000" rows="5" class="input-outlined mt-1 w-full"></textarea>
      </label>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <button type="submit" class="btn-filled">{{ pt('save') }}</button>
    </form>
  </div>
</template>
