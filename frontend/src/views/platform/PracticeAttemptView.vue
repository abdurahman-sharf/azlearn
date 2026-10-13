<script setup lang="ts">
import { ref, computed, onMounted, nextTick, watch } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { useRouteParam } from '@/composables/useRouteParam'
import { usePracticeStore } from '@/stores/practice'
import { useWrongQuestionsStore } from '@/stores/wrongQuestions'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { practiceExam, type PracticeExam, type PracticeQuestion } from '@/api/platformExams'
import PageLoading from '@/components/platform/PageLoading.vue'
import { fillTemplate } from '@/utils/notificationText'
import { describeAnswer, isTrueAnswer, canonTf, normalizeChoice, type AnswerView } from '@/utils/answerCheck'
import {
  blankState, buildBank, itemOutcome, mistakes, mistakesBankId, practiceBankId, summarize, usableQuestions, type PracticeItemState, type PracticeOutcome,
} from '@/utils/practiceSession'
import { difficultyKey } from '@/utils/attemptMeta'
import { radioMove } from '@/utils/takeExam'

// Practice: the questions of an exam WITH their answers. Nothing is sent to the server and nothing is graded - the checking
// happens here with the server's own rules (utils/answerCheck.ts). The student can keep the questions (and the ones they
// got wrong) as local question banks and the wrong ones feed the wrong-questions list of the local practice mode.
const pt = usePt()
const i18n = useI18nStore()
const practiceStore = usePracticeStore()
const wrongStore = useWrongQuestionsStore()
// the exam in the address: the page follows it if it changes while this page stays mounted
const id = useRouteParam('id')
const examPage = computed(() => `/platform/assessments/${id.value}`)
const rtl = computed(() => i18n.locale === 'ar')
const LABELS = 'ABCDEFGHIJ'.split('')

const exam = ref<PracticeExam | null>(null)
const loading = ref(true)
const errorText = ref('')
/** the server's reason when practice cannot be opened: `practice_disabled` (the teacher has not allowed it) or `not_enrolled` */
const errorCode = ref('')
const index = ref(0)
const states = ref<Record<string, PracticeItemState>>({})
const finished = ref(false)
const notice = ref('')
const stemEl = ref<HTMLElement | null>(null)
const summaryHeading = ref<HTMLElement | null>(null)

const split = computed(() => usableQuestions(exam.value?.questions ?? []))
const questions = computed(() => split.value.usable)
const q = computed<PracticeQuestion | null>(() => questions.value[index.value] ?? null)
const stateOf = (qq: PracticeQuestion): PracticeItemState => states.value[qq.id] ?? blankState()
const st = computed(() => (q.value ? stateOf(q.value) : blankState()))
const outcome = computed<PracticeOutcome>(() => (q.value ? itemOutcome(q.value, st.value) : 'skipped'))
const summary = computed(() => summarize(questions.value, states.value))
const mistakeList = computed(() => mistakes(questions.value, states.value))
const labels = computed(() => ({ trueText: pt('trueLabel'), falseText: pt('falseLabel') }))

async function load() {
  loading.value = true
  errorText.value = ''
  errorCode.value = ''
  try {
    exam.value = await practiceExam(id.value)
  } catch (e) {
    errorCode.value = e instanceof PlatformError ? e.code : ''
    errorText.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
/** Another exam in the address: nothing of the previous one stays on screen. */
watch(id, () => {
  exam.value = null
  states.value = {}
  fed.clear()
  index.value = 0
  finished.value = false
  notice.value = ''
  void load()
})

// ── answering
function setAnswer(qq: PracticeQuestion, v: string) {
  const cur = stateOf(qq)
  if (cur.revealed) return
  states.value = { ...states.value, [qq.id]: { ...cur, answer: v } }
}
const isOn = (qq: PracticeQuestion, label: string, multi: boolean) => (multi ? (stateOf(qq).answer).includes(label) : stateOf(qq).answer === label)
function pick(qq: PracticeQuestion, label: string, multi: boolean) {
  if (!multi) return setAnswer(qq, label)
  const cur = new Set(stateOf(qq).answer.split(''))
  if (cur.has(label)) cur.delete(label)
  else cur.add(label)
  setAnswer(qq, [...cur].sort().join(''))
}
function radioTab(qq: PracticeQuestion, i: number, count: number): 0 | -1 {
  const cur = stateOf(qq).answer
  const chosen = cur ? LABELS.indexOf(cur) : -1
  return (chosen < 0 || chosen >= count ? i === 0 : chosen === i) ? 0 : -1
}
function onRadioKey(e: KeyboardEvent, qq: PracticeQuestion) {
  if (e.altKey || e.ctrlKey || e.metaKey) return
  const target = (e.target as HTMLElement).closest<HTMLElement>('[role="radio"]')
  if (!target) return
  const next = radioMove(e.key, Number(target.dataset.index), qq.options.length, rtl.value)
  if (next === null) return
  e.preventDefault()
  setAnswer(qq, LABELS[next]!)
  target.parentElement?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus()
}

/** The two choices of a true/false question: the question's own option texts when it has them, else the default words. */
function tfOptions(qq: PracticeQuestion): [string, string] {
  const own = qq.options.filter((o) => o.trim() !== '')
  return own.length >= 2 ? [own[0]!, own[1]!] : [pt('trueLabel'), pt('falseLabel')]
}

// ── wrong answers feed the local practice mode's wrong-questions list, the way that mode does (a wrong answer is recorded, a
// right one counts towards clearing it) - once per question and run
const fed = new Set<string>()
function feed(qq: PracticeQuestion) {
  if (fed.has(qq.id)) return
  const o = itemOutcome(qq, stateOf(qq))
  if (o === 'incorrect') wrongStore.recordWrong(practiceBankId(id.value), qq.id)
  else if (o === 'correct') wrongStore.recordCorrect(practiceBankId(id.value), qq.id)
  else return
  fed.add(qq.id)
}

function reveal() {
  const qq = q.value
  if (!qq || stateOf(qq).revealed) return
  states.value = { ...states.value, [qq.id]: { ...stateOf(qq), revealed: true } }
  feed(qq)
}
function selfCheck(right: boolean) {
  const qq = q.value
  if (!qq) return
  states.value = { ...states.value, [qq.id]: { ...stateOf(qq), revealed: true, selfCheck: right } }
  feed(qq)
}

// ── moving around
function focusStem() { void nextTick(() => stemEl.value?.focus()) }
function goTo(i: number) {
  index.value = Math.min(Math.max(0, i), Math.max(0, questions.value.length - 1))
  finished.value = false
  focusStem()
}
function finish() {
  for (const qq of questions.value) feed(qq)
  finished.value = true
  notice.value = ''
  void nextTick(() => summaryHeading.value?.focus())
}
function restart() {
  states.value = {}
  fed.clear()
  index.value = 0
  finished.value = false
  notice.value = ''
  focusStem()
}

// ── keeping the questions on this device
function saveBank() {
  const e = exam.value
  if (!e) return
  const bank = buildBank(practiceBankId(e.id), e.title, questions.value, Date.now())
  practiceStore.upsertBank(bank)
  notice.value = fillTemplate(pt('prcSaved'), { name: bank.name })
}
function saveMistakes() {
  const e = exam.value
  if (!e || !mistakeList.value.length) return
  const bank = buildBank(mistakesBankId(e.id), `${e.title} - ${pt('prcMistakesSuffix')}`, mistakeList.value, Date.now())
  practiceStore.upsertBank(bank)
  notice.value = fillTemplate(pt('prcSaved'), { name: bank.name })
}

// ── how things are written out
const modelAnswer = (qq: PracticeQuestion): AnswerView => describeAnswer(qq, qq.answer, labels.value, true)
const givenAnswer = (qq: PracticeQuestion): AnswerView => describeAnswer(qq, stateOf(qq).answer, labels.value)
const isCorrectOption = (qq: PracticeQuestion, label: string): boolean =>
  qq.type === 'true_false' ? (isTrueAnswer(canonTf(qq.answer)) ? 'A' : 'B') === label : normalizeChoice(qq.answer).includes(label)
const isChosenOption = (qq: PracticeQuestion, label: string): boolean =>
  qq.type === 'true_false' ? (stateOf(qq).answer ? (isTrueAnswer(canonTf(stateOf(qq).answer)) ? 'A' : 'B') === label : false) : normalizeChoice(stateOf(qq).answer).includes(label)
const outcomeIcon: Record<PracticeOutcome, string> = { correct: '✓', incorrect: '✗', skipped: '–', unchecked: '?' }
const outcomeText = (o: PracticeOutcome) => pt(`prcResult_${o}` as PlatformKey)
const typeLabel = (t: string) => pt(`bank_t_${t}` as PlatformKey)
const difficultyLabel = (d: string | null) => {
  const k = difficultyKey(d)
  return k ? pt(k) : d
}
</script>

<template>
  <div v-if="exam" class="max-w-2xl mx-auto pb-8" data-testid="practice-page">
    <router-link :to="examPage" class="text-body-sm underline" data-testid="practice-back">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words" dir="auto" data-testid="practice-title">{{ exam.title }}</h1>
    <p class="mt-2 mb-3 flex flex-wrap items-center gap-2 text-body-sm">
      <span class="font-semibold px-3 py-1 rounded-full" style="background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))" data-testid="practice-badge">{{ pt('prcBadge') }}</span>
      <span dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ exam.subject_name }}</span>
    </p>
    <p class="text-body-md mb-3" data-testid="practice-intro">{{ pt('prcIntro') }}</p>
    <p v-if="split.cancelled" class="text-body-sm mb-3" data-testid="practice-cancelled">{{ pt('prcCancelled') }} <span dir="ltr" class="inline-block">{{ split.cancelled }}</span></p>

    <p v-if="!questions.length" class="card-filled p-4" data-testid="practice-empty">{{ pt('prcEmpty') }}</p>

    <!-- one question at a time -->
    <template v-else-if="!finished && q">
      <p class="text-body-sm mb-2" data-testid="practice-progress">{{ pt('question') }} <span dir="ltr" class="inline-block">{{ index + 1 }} / {{ questions.length }}</span></p>
      <div class="card-elevated p-5 space-y-4">
        <div class="flex flex-wrap gap-2 text-xs font-semibold">
          <span class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ typeLabel(q.type) }}</span>
          <span v-if="q.difficulty" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))"><span class="sr-only">{{ pt('atmDifficulty') }}: </span>{{ difficultyLabel(q.difficulty) }}</span>
          <span v-if="q.chapter" class="px-2 py-0.5 rounded-full" dir="auto" style="background-color: rgb(var(--md-surface-container-high))"><span class="sr-only">{{ pt('atmChapter') }}: </span>{{ q.chapter }}</span>
        </div>
        <p id="prc-stem" ref="stemEl" tabindex="-1" class="text-body-lg whitespace-pre-wrap break-words focus:outline-none scroll-mt-24" dir="auto" data-testid="practice-question">{{ q.stem }}</p>

        <div v-if="q.type === 'single_choice'" class="space-y-2" role="radiogroup" aria-labelledby="prc-stem" @keydown="onRadioKey($event, q)">
          <button
            v-for="(o, i) in q.options" :key="i" type="button" role="radio" class="w-full text-start card-filled p-3 flex items-start gap-2" :class="{ 'ring-2': isOn(q, LABELS[i]!, false) }"
            :aria-checked="isOn(q, LABELS[i]!, false)" :tabindex="radioTab(q, i, q.options.length)" :data-index="i" :disabled="st.revealed" :data-testid="`practice-option-${LABELS[i]}`" @click="pick(q, LABELS[i]!, false)"
          >
            <span dir="ltr" class="font-bold inline-block shrink-0">{{ LABELS[i] }}.</span><span dir="auto" class="flex-1 min-w-0 break-words">{{ o }}</span>
            <span v-if="st.revealed && isCorrectOption(q, LABELS[i]!)" class="shrink-0 font-bold" data-testid="practice-option-correct">✓ {{ pt('prcOptCorrect') }}</span>
            <span v-else-if="st.revealed && isChosenOption(q, LABELS[i]!)" class="shrink-0 font-bold">✗ {{ pt('yourAnswer') }}</span>
            <span v-else-if="isOn(q, LABELS[i]!, false)" aria-hidden="true" class="shrink-0 font-bold">●</span>
          </button>
        </div>
        <div v-else-if="q.type === 'multi_choice'" class="space-y-2" role="group" aria-labelledby="prc-stem">
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('tkxMultiHint') }}</p>
          <button
            v-for="(o, i) in q.options" :key="i" type="button" class="w-full text-start card-filled p-3 flex items-start gap-2" :class="{ 'ring-2': isOn(q, LABELS[i]!, true) }"
            :aria-pressed="isOn(q, LABELS[i]!, true)" :disabled="st.revealed" :data-testid="`practice-option-${LABELS[i]}`" @click="pick(q, LABELS[i]!, true)"
          >
            <span dir="ltr" class="font-bold inline-block shrink-0">{{ LABELS[i] }}.</span><span dir="auto" class="flex-1 min-w-0 break-words">{{ o }}</span>
            <span v-if="st.revealed && isCorrectOption(q, LABELS[i]!)" class="shrink-0 font-bold">✓ {{ pt('prcOptCorrect') }}</span>
            <span v-else-if="st.revealed && isChosenOption(q, LABELS[i]!)" class="shrink-0 font-bold">✗ {{ pt('yourAnswer') }}</span>
            <span v-else-if="isOn(q, LABELS[i]!, true)" aria-hidden="true" class="shrink-0 font-bold">●</span>
          </button>
        </div>
        <div v-else-if="q.type === 'true_false'" class="flex flex-col sm:flex-row gap-2" role="group" aria-labelledby="prc-stem">
          <button
            v-for="(label, i) in tfOptions(q)" :key="i" type="button" class="flex-1 text-start sm:text-center" :class="isOn(q, LABELS[i]!, false) ? 'btn-filled' : 'btn-outlined'"
            :aria-pressed="isOn(q, LABELS[i]!, false)" :disabled="st.revealed" :data-testid="`practice-option-${LABELS[i]}`" @click="pick(q, LABELS[i]!, false)"
          >
            <span dir="auto">{{ label }}</span>
            <span v-if="st.revealed && isCorrectOption(q, LABELS[i]!)"> ✓</span>
            <span v-else-if="st.revealed && isChosenOption(q, LABELS[i]!)"> ✗</span>
          </button>
        </div>
        <input
          v-else-if="q.type === 'fill_blank'" aria-labelledby="prc-stem" :value="st.answer" maxlength="500" dir="auto" class="input-outlined w-full" :disabled="st.revealed"
          data-testid="practice-input" @input="setAnswer(q, ($event.target as HTMLInputElement).value)"
        />
        <textarea
          v-else aria-labelledby="prc-stem" :value="st.answer" maxlength="2000" rows="5" dir="auto" class="input-outlined w-full" :disabled="st.revealed"
          data-testid="practice-input" @input="setAnswer(q, ($event.target as HTMLTextAreaElement).value)"
        ></textarea>

        <p v-if="st.revealed" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="practice-locked">{{ pt('prcLocked') }}</p>
        <button v-else type="button" class="btn-tonal" data-testid="practice-reveal" @click="reveal">{{ pt('prcReveal') }}</button>

        <!-- the model answer, its explanation and the result of the check; announced politely when it appears -->
        <div aria-live="polite">
          <section v-if="st.revealed" class="card-filled p-4 space-y-2" :aria-label="pt('prcAnswerTitle')" data-testid="practice-answer">
            <p class="font-bold" :data-outcome="outcome" data-testid="practice-result">
              <span aria-hidden="true">{{ outcomeIcon[outcome] }}</span> {{ outcomeText(outcome) }}
            </p>
            <div v-if="outcome === 'unchecked'" class="flex flex-wrap gap-2">
              <button type="button" class="btn-tonal" data-testid="practice-self-right" @click="selfCheck(true)">{{ pt('prcSelfRight') }}</button>
              <button type="button" class="btn-outlined" data-testid="practice-self-wrong" @click="selfCheck(false)">{{ pt('prcSelfWrong') }}</button>
            </div>
            <div v-if="givenAnswer(q).kind !== 'none'" class="text-body-sm" data-testid="practice-your-answer">
              <span class="font-semibold">{{ pt('yourAnswer') }}:</span>
              <template v-if="givenAnswer(q).kind === 'choice'">
                <span v-for="(it, k) in (givenAnswer(q) as Extract<AnswerView, { kind: 'choice' }>).items" :key="k" class="ms-2 inline-block"><span dir="ltr" class="font-bold">{{ it.letter }}.</span> <span dir="auto">{{ it.text }}</span></span>
              </template>
              <span v-else dir="auto" class="ms-2 whitespace-pre-wrap break-words">{{ (givenAnswer(q) as Extract<AnswerView, { kind: 'text' }>).text }}</span>
            </div>
            <div class="text-body-sm" data-testid="practice-model-answer">
              <span class="font-semibold">{{ pt('correctAnswer') }}:</span>
              <template v-if="modelAnswer(q).kind === 'choice'">
                <span v-for="(it, k) in (modelAnswer(q) as Extract<AnswerView, { kind: 'choice' }>).items" :key="k" class="ms-2 inline-block"><span dir="ltr" class="font-bold">{{ it.letter }}.</span> <span dir="auto">{{ it.text }}</span></span>
              </template>
              <span v-else-if="modelAnswer(q).kind === 'text'" dir="auto" class="ms-2 whitespace-pre-wrap break-words">{{ (modelAnswer(q) as Extract<AnswerView, { kind: 'text' }>).text }}</span>
            </div>
            <div v-if="q.analysis" class="text-body-sm" data-testid="practice-analysis"><span class="font-semibold">{{ pt('explanation') }}:</span> <span dir="auto" class="whitespace-pre-wrap break-words">{{ q.analysis }}</span></div>
          </section>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-2 mt-4">
        <button type="button" class="btn-outlined" :disabled="index === 0" data-testid="practice-prev" @click="goTo(index - 1)">{{ pt('previous') }}</button>
        <button type="button" class="btn-outlined" :disabled="index >= questions.length - 1" data-testid="practice-next" @click="goTo(index + 1)">{{ pt('next') }}</button>
        <button type="button" class="btn-filled ms-auto" data-testid="practice-finish" @click="finish">{{ pt('prcFinish') }}</button>
      </div>
    </template>

    <!-- the summary: what happened, clearly marked as not graded -->
    <section v-else-if="finished" class="card-elevated p-5 space-y-4" aria-labelledby="prc-summary-title" data-testid="practice-summary">
      <h2 id="prc-summary-title" ref="summaryHeading" tabindex="-1" class="text-title-lg font-bold focus:outline-none">{{ pt('prcSummaryTitle') }}</h2>
      <p class="text-body-md font-semibold" data-testid="practice-not-graded">{{ pt('prcNotGraded') }}</p>
      <dl class="grid grid-cols-2 sm:grid-cols-4 gap-2">
        <div class="card-filled p-3 text-center" data-testid="practice-count-correct"><dt class="text-body-sm"><span aria-hidden="true">✓</span> {{ pt('prcCorrect') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ summary.correct }}</dd></div>
        <div class="card-filled p-3 text-center" data-testid="practice-count-incorrect"><dt class="text-body-sm"><span aria-hidden="true">✗</span> {{ pt('prcIncorrect') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ summary.incorrect }}</dd></div>
        <div class="card-filled p-3 text-center" data-testid="practice-count-skipped"><dt class="text-body-sm"><span aria-hidden="true">–</span> {{ pt('prcSkipped') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ summary.skipped }}</dd></div>
        <div v-if="summary.unchecked" class="card-filled p-3 text-center" data-testid="practice-count-unchecked"><dt class="text-body-sm"><span aria-hidden="true">?</span> {{ pt('prcUnchecked') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ summary.unchecked }}</dd></div>
      </dl>

      <ul class="space-y-1" :aria-label="pt('tkQuestionNav')">
        <li v-for="(qq, i) in questions" :key="qq.id">
          <button type="button" class="btn-text w-full justify-start text-start" :aria-label="`${pt('question')} ${i + 1}: ${outcomeText(itemOutcome(qq, stateOf(qq)))}`" :data-testid="`practice-review-${i}`" @click="goTo(i)">
            <span dir="ltr" class="inline-block w-8 shrink-0">{{ i + 1 }}.</span><span aria-hidden="true" class="me-2">{{ outcomeIcon[itemOutcome(qq, stateOf(qq))] }}</span><span dir="auto" class="truncate">{{ qq.stem }}</span>
          </button>
        </li>
      </ul>

      <div class="flex flex-wrap gap-2">
        <button type="button" class="btn-filled" data-testid="practice-save-bank" @click="saveBank">{{ pt('prcSaveBank') }}</button>
        <button type="button" class="btn-tonal" :disabled="!mistakeList.length" data-testid="practice-save-mistakes" @click="saveMistakes">{{ pt('prcSaveMistakes') }}</button>
        <button type="button" class="btn-outlined" data-testid="practice-restart" @click="restart">{{ pt('prcRestart') }}</button>
        <router-link :to="examPage" class="btn-text" data-testid="practice-back-exam">{{ pt('prcBackToExam') }}</router-link>
      </div>
      <p v-if="!mistakeList.length" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="practice-no-mistakes">{{ pt('prcNoMistakes') }}</p>
      <p v-if="notice" class="text-body-sm font-semibold" role="status" data-testid="practice-saved">
        <span dir="auto">{{ notice }}</span> <router-link to="/practice" class="underline" data-testid="practice-open-practice">{{ pt('prcOpenPractice') }}</router-link>
      </p>
    </section>
  </div>

  <PageLoading v-else-if="loading" />

  <!-- practice cannot be opened: say why, and the way back -->
  <div v-else class="max-w-2xl mx-auto space-y-3">
    <router-link :to="examPage" class="text-body-sm underline" data-testid="practice-back">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('prcBadge') }}</h1>
    <p v-if="errorCode === 'practice_disabled'" class="card-filled p-4" role="status" data-testid="practice-disabled">{{ pt('prcDisabled') }}</p>
    <p v-else-if="errorCode === 'not_enrolled'" class="card-filled p-4" role="status" data-testid="practice-not-enrolled">{{ pt('prcNotEnrolled') }}</p>
    <!-- 409 attempt_running: the key is not handed out during the student's own sitting; 403 practice_not_available: not before the exam opens or while results are withheld -->
    <template v-else-if="errorCode === 'attempt_running' || errorCode === 'practice_not_available'">
      <p class="card-filled p-4" role="status" data-testid="practice-later">{{ errorText }}</p>
      <router-link v-if="errorCode === 'attempt_running'" :to="`${examPage}/take`" class="btn-tonal inline-flex" data-testid="practice-resume">{{ pt('resumeAssessment') }}</router-link>
    </template>
    <p v-else role="alert" style="color: rgb(var(--md-error))" data-testid="practice-error">{{ errorText }}</p>
    <router-link :to="examPage" class="btn-filled inline-flex" data-testid="practice-back-exam">{{ pt('prcBackToExam') }}</router-link>
  </div>
</template>
