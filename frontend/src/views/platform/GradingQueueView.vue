<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { pendingExams, type PendingExam } from '@/api/platformGrading'

const pt = usePt()
const i18n = useI18nStore()
const auth = useAuthStore()
const items = ref<PendingExam[]>([])
// The subjects of the UNFILTERED list: the select keeps offering every subject while one of them is chosen.
const subjects = ref<{ id: string; name: string }[]>([])
const subject = ref('')
const loaded = ref(false)
const loading = ref(false)
const error = ref('')
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

function collectSubjects(rows: PendingExam[]) {
  const seen = new Map<string, string>()
  for (const e of rows) if (!seen.has(e.subject_id)) seen.set(e.subject_id, e.subject_name)
  subjects.value = [...seen].map(([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name))
}

// Answers to the latest request only: switching subjects quickly must not let a slow earlier answer overwrite the list.
let latest = 0
async function load() {
  const mine = ++latest
  loading.value = true
  error.value = ''
  try {
    const rows = await pendingExams(subject.value || undefined)
    if (mine !== latest) return
    items.value = rows
    // an unfiltered answer is the only one that knows every subject
    if (!subject.value) collectSubjects(rows)
  } catch (e) {
    if (mine !== latest) return
    error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === latest) {
      loading.value = false
      loaded.value = true
    }
  }
}
onMounted(load)

const empty = computed(() => loaded.value && !error.value && !items.value.length)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-12 space-y-4" data-testid="grading-queue">
    <router-link v-if="error" to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('gdQueue') }}</h1>
    <div v-if="subjects.length || subject" class="flex items-center gap-2 flex-wrap">
      <label for="queue-subject-select" class="text-body-sm font-semibold">{{ pt('gdSubjectFilter') }}</label>
      <select id="queue-subject-select" v-model="subject" class="input-outlined max-w-full" data-testid="queue-subject" @change="load">
        <option value="">{{ pt('gdAllSubjects') }}</option>
        <option v-for="s in subjects" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </div>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-else-if="empty" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="queue-empty">{{ subject ? pt('gdQueueNoMatch') : pt('gdQueueEmpty') }}</p>
    <ul class="space-y-3">
      <li v-for="e in items" :key="e.assessment_id" class="card-filled p-4 flex items-center gap-3 flex-wrap" data-testid="queue-row">
        <div class="min-w-0 flex-1">
          <div class="font-bold break-words">{{ e.title }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            {{ e.subject_name }} · <span dir="ltr" class="inline-block" data-testid="queue-answers">{{ e.pending_answers }}</span> {{ pt('gdPendingAnswers') }} · <span dir="ltr" class="inline-block">{{ e.pending_attempts }}</span> {{ pt('gdPendingAttempts') }}
            · <span dir="ltr" class="inline-block" data-testid="queue-attempts">{{ e.attempts_total }}</span> {{ pt('gdSubmittedAttempts') }}
            <template v-if="e.oldest_submitted_at"> · {{ pt('gdOldest') }} {{ fmt(e.oldest_submitted_at) }}</template>
            <template v-if="auth.role === 'teacher' && !e.owned"> · <span class="font-semibold" data-testid="queue-not-owned">{{ pt('gdNotOwned') }}</span></template>
          </div>
        </div>
        <router-link :to="`/platform/assessments/${e.assessment_id}/results`" class="btn-outlined" :aria-label="`${pt('gdResultsLink')}: ${e.title}`" data-testid="queue-results">{{ pt('gdResultsLink') }}</router-link>
        <router-link :to="`/platform/grading/${e.assessment_id}`" class="btn-filled" :aria-label="`${pt('gdGrade')}: ${e.title}`" data-testid="queue-grade">{{ pt('gdGrade') }}</router-link>
      </li>
    </ul>
  </div>
</template>
