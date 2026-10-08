<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { pendingExams, type PendingExam } from '@/api/platformGrading'

const pt = usePt()
const i18n = useI18nStore()
const items = ref<PendingExam[]>([])
const loaded = ref(false)
const error = ref('')
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

onMounted(async () => {
  try {
    items.value = await pendingExams()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loaded.value = true
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-12 space-y-4" data-testid="grading-queue">
    <router-link v-if="error" to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('gdQueue') }}</h1>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-else-if="loaded && !items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="queue-empty">{{ pt('gdQueueEmpty') }}</p>
    <ul class="space-y-3">
      <li v-for="e in items" :key="e.assessment_id" class="card-filled p-4 flex items-center gap-3 flex-wrap" data-testid="queue-row">
        <div class="min-w-0 flex-1">
          <div class="font-bold break-words">{{ e.title }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            {{ e.subject_name }} · <span dir="ltr" class="inline-block" data-testid="queue-answers">{{ e.pending_answers }}</span> {{ pt('gdPendingAnswers') }} · <span dir="ltr" class="inline-block">{{ e.pending_attempts }}</span> {{ pt('gdPendingAttempts') }}
            <template v-if="e.oldest_submitted_at"> · {{ pt('gdOldest') }} {{ fmt(e.oldest_submitted_at) }}</template>
          </div>
        </div>
        <router-link :to="`/platform/grading/${e.assessment_id}`" class="btn-filled" data-testid="queue-grade">{{ pt('gdGrade') }}</router-link>
      </li>
    </ul>
  </div>
</template>
