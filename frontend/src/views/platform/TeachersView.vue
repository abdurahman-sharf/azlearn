<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { listTeachers, type TeacherCard } from '@/api/platformLearning'

const pt = usePt()
const q = ref('')
const teachers = ref<TeacherCard[]>([])
const error = ref('')

async function load() {
  error.value = ''
  try {
    teachers.value = await listTeachers(q.value.trim() || undefined)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('browseTeachers') }}</h1>
    <form class="mb-4" @submit.prevent="load">
      <input v-model="q" type="search" :placeholder="pt('searchTeachers')" class="input-outlined w-full" />
    </form>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!teachers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-3">
      <li v-for="t in teachers" :key="t.id">
        <router-link :to="`/platform/teachers/${t.id}`" class="card-filled block p-4">
          <div class="font-bold">{{ t.full_name }}</div>
          <div v-if="t.bio" class="text-body-sm line-clamp-2" style="color: rgb(var(--md-on-surface-variant))">{{ t.bio }}</div>
        </router-link>
      </li>
    </ul>
  </div>
</template>
