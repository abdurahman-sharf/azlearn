<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { search, type SearchResults, type Hit } from '@/api/platformOps'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const q = ref((route.query.q as string) ?? '')
const results = ref<SearchResults | null>(null)
const error = ref('')

const groups: { key: keyof SearchResults; label: PlatformKey }[] = [
  { key: 'subjects', label: 'subjectsLabel' }, { key: 'teachers', label: 'teachersLabel' },
  { key: 'courses', label: 'coursesLabel' }, { key: 'posts', label: 'postsLabel' },
]

async function run() {
  error.value = ''
  results.value = null
  if (!q.value.trim()) return
  router.replace({ query: { q: q.value.trim() } })
  try {
    results.value = await search(q.value.trim())
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
const empty = () => results.value && groups.every(g => !results.value![g.key].length)
onMounted(run)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('searchTitle') }}</h1>
    <form class="mb-4" @submit.prevent="run">
      <input v-model="q" type="search" maxlength="60" :placeholder="pt('searchPlaceholder')" class="input-outlined w-full" data-testid="search-input" />
    </form>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="empty()" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <template v-if="results">
      <section v-for="g in groups" :key="g.key" v-show="results[g.key].length" class="mb-5">
        <h2 class="text-title-md font-bold mb-2">{{ pt(g.label) }}</h2>
        <ul class="space-y-2">
          <li v-for="h in (results[g.key] as Hit[])" :key="h.id">
            <router-link :to="h.link" class="card-filled block p-3">
              <div class="font-bold break-words">{{ h.title }}</div>
              <div v-if="h.subtitle" class="text-body-sm truncate" style="color: rgb(var(--md-on-surface-variant))">{{ h.subtitle }}</div>
            </router-link>
          </li>
        </ul>
      </section>
    </template>
  </div>
</template>
