<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { listInstitutions, type Institution } from '@/api/platformAdmin'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()

const institutions = ref<Institution[]>([])
const error = ref('')

onMounted(async () => {
  if (auth.role === 'admin') return
  try {
    // Teachers/students see the institutions of the type they registered with.
    institutions.value = await listInstitutions(auth.profile?.institution_type ?? undefined)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

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

    <section v-if="auth.role === 'admin'" class="space-y-3 mb-6">
      <h2 class="text-title-md font-bold">{{ pt('adminPanel') }}</h2>
      <router-link to="/platform/admin/users" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminUsers') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminUsersDesc') }}</div>
      </router-link>
      <router-link to="/platform/admin/institutions" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminInstitutions') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminInstitutionsDesc') }}</div>
      </router-link>
    </section>

    <section v-else class="mb-6">
      <h2 class="text-title-md font-bold mb-3">{{ pt('yourInstitutions') }}</h2>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-else-if="!institutions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noInstitutions') }}</p>
      <ul class="space-y-3">
        <li v-for="i in institutions" :key="i.id" class="card-filled p-4">
          <div class="font-bold">{{ i.name_ar }}</div>
          <div v-if="i.name_en || i.city" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ [i.name_en, i.city].filter(Boolean).join(' · ') }}</div>
        </li>
      </ul>
    </section>

    <button class="btn-outlined" @click="logout">{{ pt('logout') }}</button>
  </div>
</template>
