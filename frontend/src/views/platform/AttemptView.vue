<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePracticeStore } from '@/stores/practice'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { getAttempt, gradeAttempt, type AttemptResult } from '@/api/platformExams'
import type { Question, QuestionType } from '@exameow/shared'

const pt = usePt()
const i18n = useI18nStore()
const auth = useAuthStore()
const practice = usePracticeStore()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string

const r = ref<AttemptResult | null>(null)
const error = ref('')
const msg = ref('')
const grades = ref<Record<string, number>>({})
const LABELS = 'ABCDEFGHIJ'.split('')

const isGrader = computed(() => auth.role === 'teacher' || auth.role === 'admin')
const pendingItems = computed(() => r.value?.items?.filter(i => i.correct === null) ?? [])

async function load() {
  try {
    r.value = await getAttempt(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function saveGrades() {
  error.value = ''
  const payload = Object.fromEntries(Object.entries(grades.value).filter(([, v]) => v !== undefined && !Number.isNaN(v)))
  try {
    r.value = await gradeAttempt(id, payload)
    grades.value = {}
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

/** Turns the reviewed assessment into a local practice bank (uses the existing practice mode). */
function practiceThese() {
  if (!r.value?.items) return
  const questions: Question[] = r.value.items.map(i => ({
    id: i.id, type: i.type as QuestionType, stem: i.stem, options: i.options,
    answer: i.correct_answer ?? '', analysis: i.analysis ?? '',
  }))
  practice.addBank({ id: `platform-${r.value.assessment_id}-${Date.now().toString(36)}`, name: r.value.title, questions, createdAt: Date.now(), source: 'ai-generated' })
  msg.value = pt('practiceAdded')
  router.push('/practice')
}

const show = (v: string | null) => (v && v.trim() ? v : pt('noAnswer'))
onMounted(load)
</script>

<template>
  <div v-if="r" class="max-w-3xl mx-auto pb-8">
    <router-link :to="isGrader ? `/platform/assessments/${r.assessment_id}/results` : `/platform/assessments/${r.assessment_id}`" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words">{{ r.title }}</h1>
    <p v-if="r.student_name" class="text-body-lg">{{ r.student_name }}</p>

    <div class="card-elevated p-5 my-4 text-center" data-testid="score-card">
      <template v-if="r.status === 'submitted' && !r.released">
        <p class="text-body-lg" data-testid="not-released">{{ pt('tkNotReleased') }}</p>
        <p v-if="r.release_at" class="text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt('tkReleaseAt') }}: {{ new Date(r.release_at).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' }) }}</p>
      </template>
      <template v-else-if="r.status === 'submitted'">
        <div class="text-body-sm">{{ pt('yourScore') }}</div>
        <div class="text-display-sm font-bold" data-testid="score" dir="ltr">{{ r.score }} / {{ r.total }}</div>
        <div v-if="r.pending" class="text-body-sm mt-1" data-testid="pending">{{ pt('pendingGrading') }} ({{ r.pending }})</div>
        <div v-if="r.passed !== null" class="mt-2 font-bold" :style="{ color: r.passed ? 'rgb(var(--azl-success-text, var(--md-primary)))' : 'rgb(var(--md-error))' }" data-testid="pass-badge">
          {{ r.passed ? pt('tkPassed') : pt('tkFailed') }}<span v-if="r.pass_mark" class="text-body-sm font-normal ms-2">({{ pt('tkPassMark') }} <span dir="ltr" class="inline-block">{{ r.pass_mark }}%</span>)</span>
        </div>
        <div v-if="r.tab_leaves !== null && isGrader" class="text-body-sm mt-2" data-testid="tab-leaves">{{ pt('tkTabLeaves') }}: <span dir="ltr" class="inline-block">{{ r.tab_leaves }}</span></div>
      </template>
      <div v-else>{{ pt('expiredAttempt') }}</div>
    </div>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="msg" class="text-body-sm mb-3" role="status">{{ msg }}</p>
    <p v-if="!r.items && r.status === 'submitted' && r.released" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('answersHidden') }}</p>

    <button v-if="r.items && auth.role === 'student'" class="btn-tonal mb-4" @click="practiceThese">{{ pt('practiceThese') }}</button>

    <ol v-if="r.items" class="space-y-3">
      <li v-for="(it, i) in r.items" :key="it.id" class="card-filled p-4 space-y-2">
        <div class="flex items-start gap-2">
          <span class="font-bold">{{ i + 1 }}.</span>
          <p class="flex-1 whitespace-pre-wrap break-words" dir="auto">{{ it.stem }}</p>
          <span class="shrink-0 font-bold" :style="{ color: it.correct === true ? 'rgb(var(--md-primary))' : it.correct === false ? 'rgb(var(--md-error))' : undefined }">
            <span dir="ltr" class="inline-block">{{ it.correct === true ? '✓' : it.correct === false ? '✗' : '…' }} {{ it.points }}/{{ it.max }}</span>
          </span>
        </div>
        <ul v-if="it.options.length" class="text-body-sm space-y-0.5">
          <li v-for="(o, k) in it.options" :key="k"><span dir="ltr" class="inline-block">{{ LABELS[k] }}.</span> <span dir="auto">{{ o }}</span></li>
        </ul>
        <div class="text-body-sm"><span class="font-semibold">{{ pt('yourAnswer') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words">{{ show(it.your_answer) }}</span></div>
        <div v-if="it.correct_answer" class="text-body-sm"><span class="font-semibold">{{ pt('correctAnswer') }}:</span> <span dir="auto">{{ it.correct_answer }}</span></div>
        <div v-if="it.analysis" class="text-body-sm"><span class="font-semibold">{{ pt('explanation') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words">{{ it.analysis }}</span></div>
        <label v-if="isGrader && it.correct === null" class="block text-body-sm">
          {{ pt('gradeShort').replace('{max}', String(it.max)) }}
          <input v-model.number="grades[it.id]" type="number" min="0" :max="it.max" step="0.5" class="input-outlined mt-1 w-full" :data-testid="`grade-${it.id}`" />
        </label>
      </li>
    </ol>

    <button v-if="isGrader && pendingItems.length" class="btn-filled mt-4" data-testid="save-grades" @click="saveGrades">{{ pt('saveGrades') }}</button>
  </div>
  <div v-else-if="error" class="max-w-3xl mx-auto">
    <h1 class="sr-only">{{ pt('attemptPageTitle') }}</h1>
    <p role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
  </div>
</template>
