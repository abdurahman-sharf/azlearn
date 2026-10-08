<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { gradeBatch, gradingSheet, type GradingQuestion, type GradingSheet } from '@/api/platformGrading'
import PageError from '@/components/platform/PageError.vue'

const pt = usePt()
const route = useRoute()
const id = route.params.id as string

const sheet = ref<GradingSheet | null>(null)
const active = ref(0)
const onlyPending = ref(true)
const error = ref('')
const notice = ref('')
const saving = ref(false)
// typed grades per attempt id for the question on screen (strings so an empty box means "not graded yet")
const typed = reactive<Record<string, string>>({})

const q = computed<GradingQuestion | null>(() => sheet.value?.questions[active.value] ?? null)

function resetTyped() {
  for (const k of Object.keys(typed)) delete typed[k]
  for (const a of q.value?.answers ?? []) if (a.points !== null) typed[a.attempt_id] = String(a.points)
}

async function load(keepActive = true) {
  error.value = ''
  try {
    const s = await gradingSheet(id, onlyPending.value)
    sheet.value = s
    if (!keepActive || active.value >= s.questions.length) active.value = 0
    resetTyped()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(() => load(false))

function pick(i: number) { active.value = i; notice.value = ''; error.value = ''; resetTyped() }
async function toggle() { onlyPending.value = !onlyPending.value; await load() }
const setAll = (a: string, v: number) => { typed[a] = String(v) }

function focusNext(ev: Event) {
  const inputs = [...document.querySelectorAll<HTMLInputElement>('[data-grade-input]')]
  const i = inputs.indexOf(ev.target as HTMLInputElement)
  inputs[i + 1]?.focus()
}

async function save() {
  const question = q.value
  if (!question) return
  error.value = ''
  notice.value = ''
  const changed = question.answers
    .map((a) => ({ a, v: String(typed[a.attempt_id] ?? '').trim() })) // a number input yields a number via v-model
    .filter(({ a, v }) => v !== '' && Number(v) !== a.points)
  if (!changed.length) { error.value = pt('gdNothingToSave'); return }
  if (changed.some(({ v }) => !Number.isFinite(Number(v)) || Number(v) < 0 || Number(v) > question.max)) { error.value = pt('gdBadPoints'); return }
  saving.value = true
  try {
    await gradeBatch(id, changed.map(({ a, v }) => ({ attempt_id: a.attempt_id, question_id: question.id, points: Number(v) })))
    notice.value = pt('gdSaved')
    const here = active.value
    await load()
    // when this question has nothing left to grade, move on to the next one that still does
    if (onlyPending.value && !(sheet.value?.questions[here]?.answers.length)) {
      const next = sheet.value?.questions.findIndex((x) => x.answers.length) ?? -1
      if (next >= 0) { active.value = next; resetTyped() }
    }
    await nextTick()
    document.querySelector<HTMLInputElement>('[data-grade-input]')?.focus()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div v-if="sheet" class="max-w-3xl mx-auto pb-16 space-y-4" data-testid="grade-sheet">
    <router-link to="/platform/grading" class="text-body-sm underline">{{ pt('gdQueue') }}</router-link>
    <div>
      <h1 class="text-display-sm font-bold tracking-tight break-words">{{ pt('gdSheetTitle') }}: {{ sheet.title }}</h1>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ sheet.subject_name }}</p>
    </div>

    <p v-if="!sheet.questions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="no-short">{{ pt('gdNoShort') }}</p>
    <template v-else>
      <nav class="flex flex-wrap gap-2" :aria-label="pt('gdShortQuestions')">
        <button v-for="(x, i) in sheet.questions" :key="x.id" :class="active === i ? 'btn-filled' : 'btn-outlined'" :data-testid="'gq-' + x.id" @click="pick(i)">
          <span dir="ltr" class="inline-block">{{ i + 1 }}.</span> <span class="inline-block">({{ x.pending }})</span>
        </button>
        <label class="flex items-center gap-2 ms-auto text-body-sm"><input type="checkbox" :checked="onlyPending" data-testid="only-pending" @change="toggle" /> {{ pt('gdOnlyPending') }}</label>
      </nav>

      <section v-if="q" class="card-filled p-4 space-y-3">
        <p class="font-semibold whitespace-pre-wrap break-words" dir="auto" data-testid="gq-stem">{{ q.stem }}</p>
        <div class="card-elevated p-3 text-body-sm"><span class="font-semibold">{{ pt('gdReference') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words" data-testid="gq-reference">{{ q.reference }}</span></div>
        <p class="text-body-sm">{{ pt('gdMax') }}: <span dir="ltr" class="inline-block font-bold">{{ q.max }}</span></p>
      </section>

      <p v-if="q && !q.answers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="gq-empty">{{ q.pending === 0 && onlyPending ? pt('gdQueueEmpty') : pt('gdNoAnswers') }}</p>
      <ul v-if="q" class="space-y-3">
        <li v-for="a in q.answers" :key="a.attempt_id" class="card-filled p-4 space-y-2" data-testid="gq-answer">
          <div class="flex items-center gap-2 flex-wrap">
            <span class="font-bold">{{ a.student_name }}</span>
            <span class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ a.points === null ? pt('gdWaiting') : pt('gdGraded') }}</span>
          </div>
          <p class="whitespace-pre-wrap break-words" dir="auto" data-testid="gq-text">{{ a.answer }}</p>
          <div class="flex items-center gap-2 flex-wrap">
            <label class="flex items-center gap-2 text-body-sm">{{ pt('exScore') }}
              <input v-model="typed[a.attempt_id]" type="number" min="0" :max="q.max" step="0.5" dir="ltr" class="input-outlined w-24 !py-1" data-grade-input :data-testid="'grade-' + a.attempt_id" @keydown.enter.prevent="focusNext($event)" />
            </label>
            <button type="button" class="btn-text" @click="setAll(a.attempt_id, q.max)">{{ pt('gdFull') }}</button>
            <button type="button" class="btn-text" @click="setAll(a.attempt_id, 0)">{{ pt('gdZero') }}</button>
          </div>
        </li>
      </ul>
      <div class="flex items-center gap-3 flex-wrap">
        <button class="btn-filled" :disabled="saving || !q?.answers.length" data-testid="gq-save" @click="save">{{ pt('gdSave') }}</button>
        <p v-if="notice" role="status" class="text-body-md" data-testid="gq-notice">{{ notice }}</p>
        <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="gq-error">{{ error }}</p>
      </div>
    </template>
  </div>
  <PageError v-else-if="error" :message="error" back-to="/platform/grading" />
</template>
