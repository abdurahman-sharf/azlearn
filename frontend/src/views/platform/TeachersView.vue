<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { listTeachers, type TeacherCard } from '@/api/platformLearning'
import PageLoading from '@/components/platform/PageLoading.vue'

const pt = usePt()
const q = ref('')
const teachers = ref<TeacherCard[]>([])
const error = ref('')
// "No results" only once an answer is in (never while the first list is still on the way)
const loading = ref(true)
let seq = 0

async function load() {
  const mine = ++seq
  error.value = ''
  loading.value = true
  try {
    const list = await listTeachers(q.value.trim() || undefined)
    if (mine === seq) teachers.value = list
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === seq) loading.value = false
  }
}
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('browseTeachers') }}</h1>
    <form class="mb-4" @submit.prevent="load">
      <input v-model="q" type="search" :placeholder="pt('searchTeachers')" :aria-label="pt('searchTeachers')" class="input-outlined w-full" data-testid="teachers-search" />
    </form>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <PageLoading v-if="loading && !teachers.length" :rows="2" />
    <p v-else-if="!loading && !error && !teachers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="teachers-empty">{{ pt('noResults') }}</p>
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
