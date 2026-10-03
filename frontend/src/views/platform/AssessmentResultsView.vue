<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { assessmentResults, type ResultsSummary } from '@/api/platformExams'

const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const id = route.params.id as string
const data = ref<ResultsSummary | null>(null)
const error = ref('')
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

onMounted(async () => {
  try {
    data.value = await assessmentResults(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})
</script>

<template>
  <div v-if="data" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/assessments/${id}`" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3 break-words">{{ data.info.title }} — {{ pt('results') }}</h1>
    <div class="grid grid-cols-4 gap-2 mb-4 text-center">
      <div class="card-filled p-2"><div class="text-body-sm">{{ pt('submittedCount') }}</div><div class="font-bold" data-testid="n-submitted">{{ data.submitted }}</div></div>
      <div class="card-filled p-2"><div class="text-body-sm">{{ pt('average') }}</div><div class="font-bold" data-testid="avg">{{ data.average }}</div></div>
      <div class="card-filled p-2"><div class="text-body-sm">{{ pt('highest') }}</div><div class="font-bold">{{ data.highest }}</div></div>
      <div class="card-filled p-2"><div class="text-body-sm">{{ pt('lowest') }}</div><div class="font-bold">{{ data.lowest }}</div></div>
    </div>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!data.attempts.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-2">
      <li v-for="a in data.attempts" :key="a.attempt_id">
        <router-link :to="`/platform/attempts/${a.attempt_id}`" class="card-filled p-3 flex items-center gap-2">
          <span class="flex-1 min-w-0 truncate font-bold">{{ a.student_name }}</span>
          <span v-if="a.status === 'expired'" class="text-body-sm">{{ pt('expiredAttempt') }}</span>
          <template v-else>
            <span v-if="a.pending" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-secondary-container))">{{ pt('pendingGrading') }}</span>
            <span class="font-bold" dir="ltr">{{ a.score }} / {{ data.info.total_points }}</span>
          </template>
          <span v-if="a.submitted_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(a.submitted_at) }}</span>
        </router-link>
      </li>
    </ul>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
