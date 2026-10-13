<script setup lang="ts">
import { ref, reactive, computed, onMounted, onBeforeUnmount } from 'vue'
import { useRoute } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePracticeStore } from '@/stores/practice'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, gradingErrorMessage } from '@/i18n/platform'
import { getAttempt, gradeAttempt, myAttempts, type AttemptResult, type ItemResult } from '@/api/platformExams'
import { PlatformError } from '@/lib/platformApi'
import GradeInput from '@/components/platform/GradeInput.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import { fillTemplate } from '@/utils/notificationText'
import { describeAnswer, isBlank, normalizeChoice } from '@/utils/answerCheck'
import { difficultyKey, formatDuration, hasAttemptMeta, releaseNotice } from '@/utils/attemptMeta'
import { examClock } from '@/utils/serverClock'
import { PRACTICE_GRACE_MS as RELEASE_GRACE_MS } from '@/utils/practiceGate'
import { buildItems, mergeSheet, sheetFromAttempt, toAttemptBody, validateItems, type CellMap, type GradeCell, type ServerMap } from '@/utils/gradeSheet'
import type { Question, QuestionType } from '@exameow/shared'

const pt = usePt()
const i18n = useI18nStore()
const auth = useAuthStore()
const practice = usePracticeStore()
const route = useRoute()
const id = route.params.id as string

const r = ref<AttemptResult | null>(null)
const error = ref('')
const msg = ref('')
const saving = ref(false)
const LABELS = 'ABCDEFGHIJ'.split('')

const isGrader = computed(() => auth.role === 'teacher' || auth.role === 'admin')
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

// A grader can write points and a comment on EVERY written answer of the attempt, also on one that was graded already
// (a correction). `state` holds what is typed, `base` what the server has; only the differences are sent.
const state = reactive<CellMap>(new Map())
const base = reactive<ServerMap>(new Map())
const changes = computed(() => buildItems(state, base))
/** Written, non-blank, counted answers: the only ones the server accepts a grade or a comment for (anything else is `not_gradable`). */
const editable = (it: ItemResult): boolean =>
  isGrader.value && it.type === 'short_answer' && it.max > 0 && (it.your_answer ?? '').trim() !== ''
/** "Graded by Sara on 3 May": the grader's name may be gone (a deleted account), the time stays. */
const gradedBy = (it: ItemResult): string =>
  it.graded_by_name ? fillTemplate(pt('gdGradedBy'), { name: it.graded_by_name, date: fmt(it.graded_at ?? 0) }) : fillTemplate(pt('gdGradedAt'), { date: fmt(it.graded_at ?? 0) })
const editableItems = computed(() => r.value?.items?.filter(editable) ?? [])
const hasGraded = computed(() => editableItems.value.some((it) => it.correct !== null))
const cellOf = (qid: string): GradeCell => state.get(qid)?.get(r.value?.attempt_id ?? '') ?? { points: '', feedback: '' }
const setCell = (qid: string, v: GradeCell) => { state.get(qid)?.set(r.value?.attempt_id ?? '', v) }

function seed(res: AttemptResult) {
  state.clear()
  base.clear()
  if (res.items) mergeSheet(state, base, sheetFromAttempt(res.attempt_id, res.items.filter(editable)))
}

/**
 * The attempt is still running (409 `in_progress`, or a result whose status says so): its own student gets a message with
 * a way back into it and a way back to the exam page - never a jump into another mode.
 */
const resumeLink = ref('')
const examLink = ref('')
const inProgress = ref(false)
function lookUpExam() {
  // the address only names the attempt; my attempts list says which exam it belongs to
  myAttempts()
    .then((list) => {
      const mine = list.find((t) => t.attempt_id === id)
      if (mine) {
        resumeLink.value = `/platform/assessments/${mine.assessment_id}/take`
        examLink.value = `/platform/assessments/${mine.assessment_id}`
      }
    })
    .catch(() => { /* the message alone still explains it */ })
}
// A result withheld until the exam closes appears once the last attempt could have landed (the closing time plus the grace).
// A student handed in by the hard end lands here a moment after the closing time, inside that window: the page says "shortly"
// instead of a closing time that has already passed, and looks again once by itself.
let releaseTimer: ReturnType<typeof setTimeout> | null = null
let releaseTries = 0
const RELEASE_LOOKAHEAD_MS = 10 * 60_000
const RELEASE_TRIES = 6
function watchRelease(res: AttemptResult) {
  if (releaseTimer) clearTimeout(releaseTimer)
  releaseTimer = null
  if (res.status !== 'submitted' || res.released || res.release_at === null || releaseTries >= RELEASE_TRIES) return
  const wait = res.release_at + RELEASE_GRACE_MS + 2000 - examClock.now()
  if (wait > RELEASE_LOOKAHEAD_MS) return // far away: nothing to wait for on this page
  releaseTries++
  releaseTimer = setTimeout(() => { releaseTimer = null; void load() }, Math.max(wait, 5000))
}
const release = computed(() => (r.value ? releaseNotice(r.value.release_at, examClock.now()) : 'none'))
async function load() {
  try {
    r.value = await getAttempt(id)
    seed(r.value)
    watchRelease(r.value)
    if (r.value.status === 'in_progress') {
      inProgress.value = true
      examLink.value = `/platform/assessments/${r.value.assessment_id}`
      resumeLink.value = `/platform/assessments/${r.value.assessment_id}/take`
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    if (e instanceof PlatformError && e.code === 'in_progress' && auth.role === 'student') {
      inProgress.value = true
      lookUpExam()
    }
  }
}

async function saveGrades() {
  if (saving.value || !r.value) return
  error.value = ''
  msg.value = ''
  const list = changes.value
  if (!list.length) { error.value = pt('gdNothingToSave'); return }
  const bad = validateItems(list, base)
  if (bad) { error.value = bad.code === 'points' ? pt('gdBadPoints') : pt('gdFeedbackTooLong'); return }
  saving.value = true
  try {
    r.value = await gradeAttempt(id, toAttemptBody(list))
    seed(r.value)
    msg.value = pt('gdSaved')
  } catch (e) {
    error.value = gradingErrorMessage(pt, e)
  } finally {
    saving.value = false
  }
}

/** Turns the reviewed assessment into a local practice bank (uses the existing practice mode). One bank per exam: doing it again updates it. */
const practiceReady = ref(false)
function practiceThese() {
  if (!r.value?.items) return
  // A voided question (worth 0 points) was dropped because its key was wrong: practising it would grade the student
  // against that same faulty key.
  const questions: Question[] = r.value.items.filter(i => i.max > 0).map(i => ({
    id: i.id, type: i.type as QuestionType, stem: i.stem, options: i.options,
    answer: i.correct_answer ?? '', analysis: i.analysis ?? '',
  }))
  if (!questions.length) {
    msg.value = pt('practiceNoneLeft')
    practiceReady.value = false
    return
  }
  practice.upsertBank({ id: `platform-${r.value.assessment_id}`, name: r.value.title, questions, createdAt: Date.now(), source: 'ai-generated' })
  msg.value = pt('practiceAdded')
  practiceReady.value = true // the notice offers a link to the practice page; nothing jumps there by itself
}

const show = (v: string | null) => (v && v.trim() ? v : pt('noAnswer'))
const tfLabels = computed(() => ({ trueText: pt('trueLabel'), falseText: pt('falseLabel') }))
/**
 * The student's answer or the model answer, written as letters plus the option text (the server's `your_answer_text` when it
 * sends one, else the text found in the question's options). Null when there is nothing to show.
 */
function answerParts(it: ItemResult, which: 'yours' | 'correct'): { letters: string; text: string } | null {
  const value = which === 'yours' ? it.your_answer : it.correct_answer
  if (isBlank(value)) return null
  const v = describeAnswer(it, value, tfLabels.value, which === 'correct')
  if (v.kind === 'none') return null
  const given = which === 'yours' && it.your_answer_text && it.your_answer_text.trim() ? it.your_answer_text : null
  if (v.kind === 'text') return { letters: '', text: given ?? v.text }
  return { letters: v.items.map((x) => x.letter).join(''), text: given ?? v.items.map((x) => x.text).filter(Boolean).join(' | ') }
}
const optionIsCorrect = (it: ItemResult, k: number): boolean => !!it.correct_answer && normalizeChoice(it.correct_answer).includes(LABELS[k]!)
const optionIsChosen = (it: ItemResult, k: number): boolean => !!it.your_answer && normalizeChoice(it.your_answer).includes(LABELS[k]!)
const difficultyText = (d: string | null | undefined) => {
  const k = difficultyKey(d)
  return k ? pt(k) : (d ?? '')
}
const showMeta = computed(() => !!r.value && r.value.status !== 'in_progress' && hasAttemptMeta(r.value))
const durationText = computed(() => formatDuration(r.value?.duration_sec))

// Printing the result (or saving it as a PDF through the browser's print dialog): while this page is open the print
// stylesheet below hides the app chrome, the buttons and the navigation, and keeps the colours readable on paper.
const printResult = () => window.print()
onMounted(() => {
  document.documentElement.classList.add('print-attempt')
  void load()
})
onBeforeUnmount(() => {
  document.documentElement.classList.remove('print-attempt')
  if (releaseTimer) clearTimeout(releaseTimer)
})
</script>

<template>
  <div v-if="r" class="max-w-3xl mx-auto pb-8">
    <router-link :to="isGrader ? `/platform/assessments/${r.assessment_id}/results` : `/platform/assessments/${r.assessment_id}`" class="text-body-sm underline no-print">{{ pt('back') }}</router-link>
    <div class="flex flex-wrap items-start gap-3 mt-3">
      <h1 class="text-display-sm font-bold tracking-tight break-words flex-1 min-w-0" dir="auto">{{ r.title }}</h1>
      <button type="button" class="btn-outlined no-print" data-testid="attempt-print" @click="printResult">{{ pt('atmPrint') }}</button>
    </div>
    <p v-if="r.student_name" class="text-body-lg" dir="auto" data-testid="attempt-student">{{ r.student_name }}</p>
    <!-- the student's own page carries no name from the server; on paper the result must say whose it is (the student's own account name) -->
    <p v-else-if="auth.role === 'student' && auth.profile?.full_name" class="text-body-lg" data-testid="attempt-student-self"><span class="font-semibold">{{ pt('atmStudent') }}:</span> <span dir="auto">{{ auth.profile.full_name }}</span></p>

    <!-- still running: no score yet; a way back into the attempt and a way back to the exam page -->
    <div v-if="r.status === 'in_progress'" class="card-elevated p-5 my-4 space-y-3" role="status" data-testid="attempt-in-progress">
      <p class="text-body-lg font-semibold">{{ pt('atmInProgressTitle') }}</p>
      <p class="text-body-md">{{ pt('atmInProgressMsg') }}</p>
      <div class="flex flex-wrap gap-2 no-print">
        <router-link v-if="resumeLink" :to="resumeLink" class="btn-filled" data-testid="attempt-resume">{{ pt('resumeAssessment') }}</router-link>
        <router-link v-if="examLink" :to="examLink" class="btn-outlined" data-testid="attempt-back-exam">{{ pt('atmBackExam') }}</router-link>
      </div>
    </div>
    <div v-else class="card-elevated p-5 my-4 text-center" data-testid="score-card">
      <template v-if="r.status === 'submitted' && !r.released">
        <p class="text-body-lg" data-testid="not-released">{{ pt('tkNotReleased') }}</p>
        <p v-if="release === 'soon'" class="text-body-sm mt-1" role="status" data-testid="release-soon">{{ pt('tkReleaseSoon') }}</p>
        <p v-else-if="r.release_at" class="text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))" data-testid="release-at">{{ pt('tkReleaseAt') }}: {{ fmt(r.release_at) }}</p>
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

    <!-- which attempt this is, when it started and ended, how long it took -->
    <section v-if="showMeta" class="card-filled p-4 mb-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-body-sm" :aria-label="pt('atmMeta')" data-testid="attempt-meta">
      <span v-if="typeof r.attempt_no === 'number'" data-testid="attempt-no"><span class="font-semibold">{{ pt('atmAttemptNo') }}:</span> <span dir="ltr" class="inline-block">{{ r.attempt_no }}</span></span>
      <span v-if="r.is_best" class="font-bold px-3 py-1 rounded-full" style="background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))" data-testid="attempt-best"><span aria-hidden="true">★</span> {{ pt('atmBest') }}</span>
      <span data-testid="attempt-started"><span class="font-semibold">{{ pt('atmStarted') }}:</span> <span dir="auto">{{ fmt(r.started_at) }}</span></span>
      <span v-if="r.submitted_at" data-testid="attempt-submitted-at"><span class="font-semibold">{{ pt('atmSubmitted') }}:</span> <span dir="auto">{{ fmt(r.submitted_at) }}</span></span>
      <span v-if="durationText" data-testid="attempt-duration"><span class="font-semibold">{{ pt('atmDuration') }}:</span> <span dir="ltr" class="inline-block">{{ durationText }}</span></span>
    </section>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="attempt-error">{{ error }}</p>
    <p v-if="msg" class="text-body-sm mb-3" role="status" data-testid="attempt-notice">{{ msg }}<template v-if="practiceReady"> <router-link to="/practice" class="underline font-semibold no-print" data-testid="attempt-practice-link">{{ pt('prcOpenPractice') }}</router-link></template></p>
    <p v-if="!r.items && r.status === 'submitted' && r.released" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('answersHidden') }}</p>

    <button v-if="r.items && auth.role === 'student'" class="btn-tonal mb-4 no-print" data-testid="attempt-practice-these" @click="practiceThese">{{ pt('practiceThese') }}</button>

    <ol v-if="r.items" class="space-y-3">
      <li v-for="(it, i) in r.items" :key="it.id" class="card-filled p-4 space-y-2">
        <div class="flex items-start gap-2">
          <span class="font-bold">{{ i + 1 }}.</span>
          <p class="flex-1 whitespace-pre-wrap break-words" dir="auto">{{ it.stem }}</p>
          <span class="shrink-0 font-bold" :data-print="it.correct === true ? 'ok' : it.correct === false ? 'bad' : undefined" :style="{ color: it.correct === true ? 'rgb(var(--azl-success-text, var(--md-primary)))' : it.correct === false ? 'rgb(var(--md-error))' : undefined }">
            <span v-if="it.max === 0" data-testid="item-voided">⊘ {{ pt('keyVoided') }}</span>
            <span v-else dir="ltr" class="inline-block">{{ it.correct === true ? '✓' : it.correct === false ? '✗' : '…' }} {{ it.points }}/{{ it.max }}</span>
          </span>
        </div>
        <div v-if="it.chapter || difficultyText(it.difficulty)" class="flex flex-wrap gap-2 text-xs font-semibold">
          <span v-if="it.chapter" class="px-2 py-0.5 rounded-full border" style="border-color: rgb(var(--md-outline))" dir="auto" data-testid="item-chapter"><span class="sr-only">{{ pt('atmChapter') }}: </span>{{ it.chapter }}</span>
          <span v-if="difficultyText(it.difficulty)" class="px-2 py-0.5 rounded-full border" style="border-color: rgb(var(--md-outline))" data-testid="item-difficulty"><span class="sr-only">{{ pt('atmDifficulty') }}: </span>{{ difficultyText(it.difficulty) }}</span>
        </div>
        <ul v-if="it.options.length" class="text-body-sm space-y-0.5">
          <li v-for="(o, k) in it.options" :key="k">
            <span dir="ltr" class="inline-block">{{ LABELS[k] }}.</span> <span dir="auto">{{ o }}</span>
            <span v-if="optionIsCorrect(it, k)" class="font-semibold" data-print="ok" data-testid="item-option-correct"> ✓ {{ pt('prcOptCorrect') }}</span>
            <span v-if="optionIsChosen(it, k)" class="font-semibold" data-testid="item-option-chosen"> ● {{ pt('yourAnswer') }}</span>
          </li>
        </ul>
        <div class="text-body-sm" data-testid="item-your-answer">
          <span class="font-semibold">{{ pt('yourAnswer') }}:</span>
          <template v-if="answerParts(it, 'yours')">
            <span v-if="answerParts(it, 'yours')!.letters" dir="ltr" class="inline-block font-bold ms-1">{{ answerParts(it, 'yours')!.letters }}</span>
            <span v-if="answerParts(it, 'yours')!.text" dir="auto" class="ms-1 whitespace-pre-wrap break-words">{{ answerParts(it, 'yours')!.text }}</span>
          </template>
          <span v-else dir="auto" class="ms-1 whitespace-pre-wrap break-words">{{ show(it.your_answer) }}</span>
        </div>
        <div v-if="it.correct_answer" class="text-body-sm" data-testid="item-correct-answer">
          <span class="font-semibold">{{ pt('correctAnswer') }}:</span>
          <template v-if="answerParts(it, 'correct')">
            <span v-if="answerParts(it, 'correct')!.letters" dir="ltr" class="inline-block font-bold ms-1">{{ answerParts(it, 'correct')!.letters }}</span>
            <span v-if="answerParts(it, 'correct')!.text" dir="auto" class="ms-1 break-words">{{ answerParts(it, 'correct')!.text }}</span>
          </template>
          <span v-else dir="auto" class="ms-1">{{ it.correct_answer }}</span>
        </div>
        <div v-if="it.analysis" class="text-body-sm"><span class="font-semibold">{{ pt('explanation') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words">{{ it.analysis }}</span></div>
        <div v-if="it.feedback && !editable(it)" class="text-body-sm" data-testid="item-feedback"><span class="font-semibold">{{ pt('gdTeacherFeedback') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words">{{ it.feedback }}</span></div>
        <p v-if="isGrader && it.graded_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="item-graded-by">{{ gradedBy(it) }}</p>
        <GradeInput
          v-if="editable(it)" :id-prefix="`at-${it.id}`" :max="it.max" :label="`${r.student_name} - ${pt('gdQuestion')} ${i + 1}`"
          :model-value="cellOf(it.id)" :points-testid="`grade-${it.id}`" :feedback-testid="`grade-feedback-${it.id}`"
          @update:model-value="setCell(it.id, $event)"
        />
      </li>
    </ol>

    <p v-if="isGrader && !r.show_answers && editableItems.length" class="text-body-sm mt-4" data-testid="item-comments-hidden">{{ pt('gdCommentsHidden') }}</p>
    <p v-if="hasGraded" class="text-body-sm mt-4" style="color: rgb(var(--md-on-surface-variant))" data-testid="correction-hint">{{ pt('gdCorrectionHint') }}</p>
    <button v-if="editableItems.length" class="btn-filled mt-4" :disabled="saving" data-testid="save-grades" @click="saveGrades">
      {{ saving ? pt('gdSaving') : pt('saveGrades') }}<template v-if="changes.length && !saving"> <span dir="ltr" class="inline-block">({{ changes.length }})</span></template>
    </button>
  </div>
  <div v-else-if="error" class="max-w-3xl mx-auto space-y-3">
    <h1 class="sr-only">{{ pt('attemptPageTitle') }}</h1>
    <p role="alert" style="color: rgb(var(--md-error))" data-testid="attempt-load-error">{{ error }}</p>
    <div v-if="inProgress" class="card-elevated p-5 space-y-3" data-testid="attempt-in-progress">
      <p class="text-body-lg font-semibold">{{ pt('atmInProgressTitle') }}</p>
      <p class="text-body-md">{{ pt('atmInProgressMsg') }}</p>
      <div class="flex flex-wrap gap-2">
        <router-link v-if="resumeLink" :to="resumeLink" class="btn-filled" data-testid="attempt-resume">{{ pt('resumeAssessment') }}</router-link>
        <router-link v-if="examLink" :to="examLink" class="btn-outlined" data-testid="attempt-back-exam">{{ pt('atmBackExam') }}</router-link>
      </div>
    </div>
  </div>
  <PageLoading v-else />
</template>

<style>
/* ── Print / save as PDF. Not scoped on purpose: it must reach the app shell around this page. It only applies while this page
   is open (the class is set on <html> by the page) and only on paper. Colour never carries the meaning alone (each result
   has a ✓ / ✗ and words), so a black-and-white printer loses nothing; the pass/fail colours below are dark enough for paper. */
@media print {
  html.print-attempt {
    color-scheme: light;
    background: #fff !important;
  }
  html.print-attempt body,
  html.print-attempt body > div,
  html.print-attempt #app > div {
    background: #fff !important;
  }
  /* the app chrome: skip link, header, offline banner, the role navigation, the mobile bar, and this page's own controls */
  html.print-attempt .skip-link,
  html.print-attempt header,
  html.print-attempt [data-testid='offline-banner'],
  html.print-attempt div[data-testid$='-shell'] > :not(main),
  html.print-attempt .safe-bottom,
  html.print-attempt main button,
  html.print-attempt main .no-print {
    display: none !important;
  }
  /* the page itself: full width, black on white, no shadows or tinted panels */
  html.print-attempt div[data-testid$='-shell'] {
    display: block !important;
    padding: 0 !important;
    max-width: none !important;
  }
  html.print-attempt main,
  html.print-attempt main * {
    color: #111 !important;
    background: transparent !important;
    box-shadow: none !important;
    text-shadow: none !important;
  }
  html.print-attempt main .card-elevated,
  html.print-attempt main .card-filled {
    border: 1px solid #9ca3af !important;
    border-radius: 8px !important;
  }
  html.print-attempt main [data-print='ok'] {
    color: #14532d !important;
  }
  html.print-attempt main [data-print='bad'] {
    color: #991b1b !important;
  }
  /* the mark sits in an inner span: it takes the colour of the element that carries data-print, not the black of the rule above */
  html.print-attempt main [data-print] * {
    color: inherit !important;
  }
  /* an item is never cut in the middle by a page break */
  html.print-attempt main li,
  html.print-attempt main section {
    break-inside: avoid;
    page-break-inside: avoid;
  }
  html.print-attempt main a {
    text-decoration: none !important;
  }
  @page {
    margin: 14mm;
  }
}
</style>
