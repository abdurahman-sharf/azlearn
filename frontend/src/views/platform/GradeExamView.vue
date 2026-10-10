<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, gradingErrorMessage } from '@/i18n/platform'
import { gradeBatch, gradingSheet, type GradingQuestion, type GradingSheet } from '@/api/platformGrading'
import PageError from '@/components/platform/PageError.vue'
import GradeInput from '@/components/platform/GradeInput.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { fillTemplate } from '@/utils/notificationText'
import {
  buildItems, chunkItems, countByQuestion, fillWaiting, keepEdited, markSaved, mergeSheet, nextWork, validateItems,
  type CellMap, type GradeCell, type ServerMap,
} from '@/utils/gradeSheet'

const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const id = route.params.id as string

const sheet = ref<GradingSheet | null>(null)
const active = ref(0)
const onlyPending = ref(true)
const error = ref('')
const notice = ref('')
const saving = ref(false)

// What the grader typed (`state`) and what the server holds (`base`), per question and attempt. Both outlive the question
// on screen: switching questions, or toggling "ungraded only", never loses a typed value.
const state = reactive<CellMap>(new Map())
const base = reactive<ServerMap>(new Map())
const items = computed(() => buildItems(state, base))
const dirtyBy = computed(() => countByQuestion(items.value))
const { markClean, asking: leaving, answer: answerLeave } = useUnsavedGuard(() => items.value)

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const q = computed<GradingQuestion | null>(() => sheet.value?.questions[active.value] ?? null)
const cellOf = (qid: string, aid: string): GradeCell => state.get(qid)?.get(aid) ?? { points: '', feedback: '' }
const setCell = (qid: string, aid: string, v: GradeCell) => { state.get(qid)?.set(aid, v) }
const isWaiting = (qid: string, aid: string) => (base.get(qid)?.get(aid)?.points ?? null) === null
const waitingOnScreen = computed(() => q.value?.answers.filter((a) => isWaiting(q.value!.id, a.attempt_id)).length ?? 0)

async function load(): Promise<boolean> {
  try {
    const s = await gradingSheet(id, onlyPending.value)
    // an answer with unsaved edits that the server no longer lists (a colleague graded it, or "ungraded only" hides it)
    // stays on screen: what Save sends is exactly what the grader can see
    keepEdited(sheet.value?.questions, s.questions, state, base)
    sheet.value = s
    if (active.value >= s.questions.length) active.value = 0
    mergeSheet(state, base, s.questions)
    // a reload while the grader has unsaved work must not turn that work into the "saved" state
    if (!items.value.length) markClean()
    return true
  } catch (e) {
    error.value = gradingErrorMessage(pt, e)
    return false
  }
}
onMounted(load)

function pick(i: number) { active.value = i; notice.value = ''; error.value = '' }
async function toggle() { onlyPending.value = !onlyPending.value; error.value = ''; await load() }
function bulk(mode: 'full' | 'zero') {
  const question = q.value
  if (!question) return
  notice.value = ''
  error.value = ''
  fillWaiting(state, base, question.id, question.answers.map((a) => a.attempt_id), mode)
}
const focusFirst = async () => { await nextTick(); document.querySelector<HTMLInputElement>('[data-grade-input]')?.focus() }

/** Saves every changed answer of every question (in calls of at most 500), then reloads. True when all of it was saved. */
async function save(): Promise<boolean> {
  if (saving.value) return false
  error.value = ''
  notice.value = ''
  const all = items.value
  if (!all.length) { error.value = pt('gdNothingToSave'); return false }
  const bad = validateItems(all, base)
  if (bad) {
    // show the question and answer concerned instead of a bare message
    const at = sheet.value?.questions.findIndex((x) => x.id === bad.question_id) ?? -1
    if (at >= 0) active.value = at
    error.value = bad.code === 'points' ? pt('gdBadPoints') : pt('gdFeedbackTooLong')
    await nextTick()
    document.querySelector<HTMLElement>(`[data-testid="${bad.code === 'points' ? 'grade-' : 'grade-feedback-'}${bad.attempt_id}"]`)?.focus()
    return false
  }
  saving.value = true
  let saved = 0
  let ok = true
  try {
    for (const part of chunkItems(all)) {
      await gradeBatch(id, part)
      markSaved(base, part)
      saved += part.length
    }
    notice.value = pt('gdSaved')
  } catch (e) {
    ok = false
    const reason = gradingErrorMessage(pt, e)
    error.value = saved > 0 ? fillTemplate(pt('gdSavedPartial'), { saved, total: all.length, reason }) : reason
  } finally {
    saving.value = false
  }
  // reload either way: the part that was saved is now in the sheet (and the pending counts moved). When the reload fails
  // too (the same outage), the report of what was and was not saved stays, with the reload failure after it.
  const report = error.value
  const reloaded = await load()
  if (!reloaded && report) error.value = `${report} ${error.value}`
  return ok
}

async function onSave() {
  const here = active.value
  if (!(await save())) return
  // when this question has nothing left to grade, move on to the next one that still has work
  if (onlyPending.value && !(sheet.value?.questions[here]?.answers.length)) {
    const next = nextWork(sheet.value?.questions ?? [], here)
    if (next >= 0) active.value = next
  }
  await focusFirst()
}

/** Ctrl/Cmd+Enter: save, then go to the next question that still has answers waiting. */
async function saveAndNext() {
  if (items.value.length && !(await save())) return
  const next = nextWork(sheet.value?.questions ?? [], active.value)
  if (next >= 0) active.value = next // not pick(): that would wipe the "saved" notice the grader just earned
  await focusFirst()
}
function onKeydown(ev: KeyboardEvent) {
  if (ev.key !== 'Enter' || !(ev.ctrlKey || ev.metaKey)) return
  // a held key repeats: one save per press. Not while an input method is composing, not on a link (Ctrl+Enter opens it
  // in a new tab) and not inside the unsaved-changes dialog (it has its own buttons).
  if (ev.repeat || ev.isComposing || saving.value) return
  if ((ev.target as HTMLElement | null)?.closest('a, [role="dialog"], [role="alertdialog"]')) return
  ev.preventDefault()
  void saveAndNext()
}
</script>

<template>
  <div v-if="sheet" class="max-w-3xl mx-auto pb-16 space-y-4" data-testid="grade-sheet" @keydown="onKeydown">
    <router-link to="/platform/grading" class="text-body-sm underline">{{ pt('gdQueue') }}</router-link>
    <div>
      <h1 class="text-display-sm font-bold tracking-tight break-words">{{ pt('gdSheetTitle') }}: {{ sheet.title }}</h1>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ sheet.subject_name }}</p>
    </div>

    <p v-if="!sheet.questions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="no-short">{{ pt('gdNoShort') }}</p>
    <template v-else>
      <nav class="flex flex-wrap gap-2" :aria-label="pt('gdShortQuestions')">
        <button v-for="(x, i) in sheet.questions" :key="x.id" :class="active === i ? 'btn-filled' : 'btn-outlined'" :aria-current="active === i ? 'true' : undefined" :data-testid="'gq-' + x.id" @click="pick(i)">
          <span dir="ltr" class="inline-block">{{ i + 1 }}.</span> <span class="inline-block">({{ x.pending }})</span>
          <template v-if="dirtyBy.get(x.id)"><span aria-hidden="true"> ●</span><span class="sr-only"> {{ pt('gdUnsavedMark') }}</span></template>
        </button>
        <label class="flex items-center gap-2 ms-auto text-body-sm"><input type="checkbox" :checked="onlyPending" data-testid="only-pending" @change="toggle" /> {{ pt('gdOnlyPending') }}</label>
      </nav>

      <p v-if="!sheet.show_answers" class="card-filled p-3 text-body-sm" data-testid="gq-comments-hidden">{{ pt('gdCommentsHidden') }}</p>
      <section v-if="q" class="card-filled p-4 space-y-3">
        <p class="font-semibold whitespace-pre-wrap break-words" dir="auto" data-testid="gq-stem">{{ q.stem }}</p>
        <div class="card-elevated p-3 text-body-sm"><span class="font-semibold">{{ pt('gdReference') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words" data-testid="gq-reference">{{ q.reference }}</span></div>
        <p class="text-body-sm">{{ pt('gdMax') }}: <span dir="ltr" class="inline-block font-bold">{{ q.max }}</span></p>
      </section>

      <p v-if="q && !q.answers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="gq-empty">{{ q.pending === 0 && onlyPending ? pt('gdQueueEmpty') : pt('gdNoAnswers') }}</p>
      <div v-if="q && q.answers.length" class="flex items-center gap-2 flex-wrap">
        <button type="button" class="btn-tonal" :disabled="!waitingOnScreen" data-testid="gq-all-full" @click="bulk('full')">{{ pt('gdAllFull') }}</button>
        <button type="button" class="btn-outlined" :disabled="!waitingOnScreen" data-testid="gq-all-zero" @click="bulk('zero')">{{ pt('gdAllZero') }}</button>
      </div>
      <ul v-if="q" class="space-y-3">
        <li v-for="a in q.answers" :key="a.attempt_id" class="card-filled p-4 space-y-2" data-testid="gq-answer">
          <div class="flex items-center gap-2 flex-wrap">
            <span class="font-bold">{{ a.student_name }}</span>
            <span class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ a.points === null ? pt('gdWaiting') : pt('gdGraded') }}</span>
            <span v-if="a.graded_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ fillTemplate(pt('gdGradedAt'), { date: fmt(a.graded_at) }) }}</span>
          </div>
          <p class="whitespace-pre-wrap break-words" dir="auto" data-testid="gq-text">{{ a.answer }}</p>
          <GradeInput
            :id-prefix="`gi-${q.id}-${a.attempt_id}`" :max="q.max" :label="`${a.student_name} - ${pt('gdQuestion')} ${active + 1}`"
            :model-value="cellOf(q.id, a.attempt_id)" :points-testid="'grade-' + a.attempt_id" :feedback-testid="'grade-feedback-' + a.attempt_id"
            @update:model-value="setCell(q.id, a.attempt_id, $event)"
          />
        </li>
      </ul>
      <div class="flex items-center gap-3 flex-wrap">
        <button class="btn-filled" :disabled="saving" data-testid="gq-save" @click="onSave">
          {{ saving ? pt('gdSaving') : pt('gdSave') }}<template v-if="items.length && !saving"> <span dir="ltr" class="inline-block" data-testid="gq-save-count">({{ items.length }})</span></template>
        </button>
        <p v-if="notice" role="status" class="text-body-md" data-testid="gq-notice">{{ notice }}</p>
        <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="gq-error">{{ error }}</p>
      </div>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('gdShortcut') }}</p>
    </template>
    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
  </div>
  <PageError v-else-if="error" :message="error" back-to="/platform/grading" />
</template>
