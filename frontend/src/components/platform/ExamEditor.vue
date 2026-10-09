<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import type { Question } from '@exameow/shared'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { useTeacherStats } from '@/lib/teacherStats'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import SubjectPicker from '@/components/platform/SubjectPicker.vue'
import BankItemForm from '@/components/platform/BankItemForm.vue'
import ExamAiPanel from '@/components/platform/ExamAiPanel.vue'
import ExamBankPicker from '@/components/platform/ExamBankPicker.vue'
import ExamImportPanel from '@/components/platform/ExamImportPanel.vue'
import ExamLocalBankPicker from '@/components/platform/ExamLocalBankPicker.vue'
import ExamPreview from '@/components/platform/ExamPreview.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'
import type { BankItem, BankItemInput } from '@/api/platformBank'
import { examApi, type ExamDetail, type ExamInput } from '@/api/platformExamAdmin'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { MAX_EXAM_QUESTIONS } from '@/utils/examImport'
import { adoptQuestions, fromLocalInput, letter, move, toLocalInput, totals, validateQuestion, type ExamQuestion, type Problem } from '@/utils/examBuilder'
import { editorState, examsSwitch } from '@/utils/examEditorState'
import { usableSubjects } from '@/utils/teachingCards'

// The exam builder shared by the admin (`mode="admin"`: server bank, AI generation, any subject) and the teacher
// (`mode="teacher"`: Excel import, manual entry and a subset of the browser's local banks, only the subjects they are
// approved for). The two differ in the API they call (/admin/exams or /teacher/exams), where they go back to, and the
// source panels shown; the form, the review step and every data-testid are the same.
const props = defineProps<{ mode: 'admin' | 'teacher' }>()
const isTeacher = props.mode === 'teacher'
const api = examApi(props.mode)
const listPath = isTeacher ? '/platform/exams' : '/platform/admin/exams'

const pt = usePt()
const route = useRoute()
const router = useRouter()

const examId = ref('')
const exam = ref<ExamDetail | null>(null)
const step = ref<1 | 2 | 3>(1)
const subjectId = ref('')
const questions = ref<ExamQuestion[]>([])
const editingId = ref('')
const showManual = ref(false)
const showPreview = ref(false)
const error = ref('')
const notice = ref('')
const saving = ref(false)
const serverProblem = ref<{ id: string; code: string } | null>(null)
/** the exam in the address could not be read (not yours, deleted, no connection): no form is shown, so nothing can be saved as a new exam by mistake */
const loadFailed = ref(false)

const f = reactive({
  title: '', description: '', duration: null as number | null, attempts: 1, opens: '', closes: '',
  shuffleQ: false, shuffleO: false, passMark: null as number | null, release: 'immediate' as 'immediate' | 'after_close', showAnswers: true,
})
const BLANK = { title: '', description: '', duration: null, attempts: 1, opens: '', closes: '', shuffleQ: false, shuffleO: false, passMark: null, release: 'immediate', showAnswers: true } as const

const isNew = computed(() => !examId.value)
/** the exam id in the address ('' on the "new" page) */
const routeId = computed(() => (typeof route.params.id === 'string' ? route.params.id : ''))
const st = computed(() => editorState(exam.value))
const frozen = computed(() => st.value.frozen)
const readOnly = computed(() => st.value.readOnly)
const locked = computed(() => st.value.contentLocked)
const sum = computed(() => totals(questions.value))
const problems = computed(() => new Map(questions.value.map((q) => [q.id, validateQuestion(q) as Problem | null])))
const firstBad = computed(() => questions.value.find((q) => problems.value.get(q.id)))
const taken = computed(() => new Set(questions.value.map((q) => q.src).filter((x): x is string => !!x)))

// ── teacher only: which subjects may be picked, and the admin's exam switch
const { stats, refresh: refreshStats } = useTeacherStats()
const examsOff = computed(() => isTeacher && stats.value?.can_create_exams === false)
const blocks = computed(() => examsSwitch(props.mode, examsOff.value, exam.value))
const teaching = ref<MyTeaching[]>([])
const teachingLoaded = ref(!isTeacher)
/** the list of subjects was read successfully (once; `teachingLoaded` only says the picker may be mounted) */
const teachingKnown = ref(false)
const teachingFailed = ref(false)
const usable = computed(() => usableSubjects(teaching.value))
const allowedIds = computed(() => usable.value.map((t) => t.subject_id))
const subjectInstitutions = computed(() => Object.fromEntries(usable.value.map((t) => [t.subject_id, t.institution_id])))

/** Reads the teacher's subjects once. `teachingLoaded` is raised by `load()` (after the subject of a link is chosen), never here, so the picker mounts with its value already set. */
async function ensureTeaching() {
  if (!isTeacher || teachingKnown.value) return
  teachingFailed.value = false
  try {
    teaching.value = await myTeaching()
    teachingKnown.value = true
  } catch (e) {
    teachingFailed.value = true
    error.value = platformErrorMessage(pt, e)
  }
}

// Unsaved changes are guarded in the teacher's editor; the admin's behaves as before (never asks), because it never marks itself clean.
// The subject is not part of the state: the picker may choose it by itself while it opens, and it is a one-click choice.
const { asking: leaving, answer: answerLeave, markClean } = useUnsavedGuard(() => (isTeacher ? { f: { ...f }, questions: questions.value } : null))
const settle = () => { if (isTeacher) markClean() }

let counter = 0
const nextId = () => `q${++counter}`
const bumpCounter = () => { counter = Math.max(counter, ...questions.value.map((q) => Number(/^q(\d+)$/.exec(q.id)?.[1] ?? 0))) }

function adopt(d: ExamDetail) {
  exam.value = d
  examId.value = d.id
  subjectId.value = d.subject_id
  questions.value = d.questions.map((q) => ({ ...q, ...(d.sources[q.id] ? { src: d.sources[q.id] } : {}) }))
  bumpCounter()
  Object.assign(f, {
    title: d.title, description: d.description ?? '', duration: d.duration_min, attempts: d.max_attempts, opens: toLocalInput(d.opens_at), closes: toLocalInput(d.closes_at),
    shuffleQ: d.shuffle_questions, shuffleO: d.shuffle_options, passMark: d.pass_mark, release: d.release_mode, showAnswers: d.show_answers,
  })
  settle()
}

async function load() {
  error.value = ''
  loadFailed.value = false
  const id = routeId.value
  if (!id) {
    // the teacher's subjects feed the picker of a NEW exam only; opening an existing one needs no such request
    await ensureTeaching()
    exam.value = null; examId.value = ''; questions.value = []; counter = 0; step.value = 1
    Object.assign(f, BLANK)
    if (isTeacher) subjectId.value = ''
    if (isTeacher && !teachingFailed.value) {
      // a link from "My subjects" (?subject=) or the only approved subject
      const wanted = typeof route.query.subject === 'string' ? route.query.subject : ''
      const pick = allowedIds.value.includes(wanted) ? wanted : allowedIds.value.length === 1 ? allowedIds.value[0] : ''
      if (pick) subjectId.value = pick
    }
    teachingLoaded.value = true
    settle()
    return
  }
  teachingLoaded.value = true
  try {
    adopt(await api.get(id))
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    loadFailed.value = true
  }
}
watch(() => route.params.id, load, { immediate: true })
/** The list of subjects could not be read: offer to read it again (the picker is not shown until it is known). */
async function retryTeaching() {
  teachingFailed.value = false
  await load()
}
onMounted(() => { if (isTeacher) refreshStats() })

// ── adding questions
function addGenerated(qs: Question[]) {
  questions.value = [...questions.value, ...adoptQuestions(qs, nextId)]
}
function addFromBank(items: BankItem[]) {
  const bankOf = new Map<Question, string>()
  const raw = items.map((i) => {
    const q = { id: '', type: i.type, stem: i.stem, options: i.options, answer: i.answer, analysis: i.analysis, chapter: i.chapter ?? undefined, difficulty: i.difficulty ?? undefined } as Question
    bankOf.set(q, i.id)
    return q
  })
  questions.value = [...questions.value, ...adoptQuestions(raw, nextId, (q) => bankOf.get(q))]
}
function addImported(qs: Question[]) {
  questions.value = [...questions.value, ...adoptQuestions(qs, nextId)]
}
function addManual(i: BankItemInput) {
  questions.value = [...questions.value, ...adoptQuestions([{ id: '', type: i.type, stem: i.stem, options: i.options, answer: i.answer, analysis: i.analysis, chapter: i.chapter ?? undefined, difficulty: i.difficulty ?? undefined } as Question], nextId)]
  showManual.value = false
}

// ── review
const asBankItem = (q: ExamQuestion): BankItem => ({ id: q.id, subject_id: subjectId.value, type: q.type, stem: q.stem, options: q.options, answer: q.answer, analysis: q.analysis, chapter: q.chapter ?? null, difficulty: q.difficulty ?? null, tags: [], created_by: null, created_at: 0, updated_at: 0, archived_at: null }) as BankItem
function saveEdit(i: BankItemInput) {
  questions.value = questions.value.map((q) => (q.id === editingId.value ? { ...q, type: i.type, stem: i.stem, options: i.options, answer: i.answer, analysis: i.analysis, chapter: i.chapter ?? undefined, difficulty: i.difficulty ?? undefined } as ExamQuestion : q))
  editingId.value = ''
}
const remove = (id: string) => { questions.value = questions.value.filter((q) => q.id !== id) }
const shift = (i: number, d: number) => { questions.value = move(questions.value, i, d) }
const setScore = (q: ExamQuestion, v: string) => { q.score = v === '' ? undefined : Number(v) }
const isCorrect = (q: ExamQuestion, i: number) => (q.type === 'single_choice' || q.type === 'multi_choice' ? q.answer.includes(letter(i)) : q.type === 'true_false' ? q.answer === letter(i) : false)
const shownOptions = (q: ExamQuestion) => (q.type === 'true_false' && !q.options.length ? ['صحيح', 'خطأ'] : q.options)

// ── saving
function payload(): ExamInput {
  const p: ExamInput = { title: f.title.trim(), description: f.description.trim(), max_attempts: Number(f.attempts), show_answers: f.showAnswers, release_mode: f.release }
  const closes = fromLocalInput(f.closes)
  if (closes !== null) p.closes_at = closes
  else if (!isNew.value) p.clear_closes = true
  if (f.passMark) p.pass_mark = Number(f.passMark)
  else if (!isNew.value) p.clear_pass_mark = true
  if (frozen.value) return p
  p.shuffle_questions = f.shuffleQ
  p.shuffle_options = f.shuffleO
  if (f.duration) p.duration_min = Number(f.duration)
  else if (!isNew.value) p.clear_duration = true
  const opens = fromLocalInput(f.opens)
  if (opens !== null) p.opens_at = opens
  else if (!isNew.value) p.clear_opens = true
  if (isNew.value) p.subject_id = subjectId.value
  p.questions = questions.value.map(({ src: _src, ...q }) => q as Question)
  p.sources = Object.fromEntries(questions.value.filter((q) => q.src).map((q) => [q.id, q.src as string]))
  return p
}

/** Errors that mean "the exam is in another state than this page thinks": the page re-reads it (without touching the form). */
const STALE = new Set(['has_attempts', 'locked', 'archived', 'invalid_transition', 'closed'])

async function save(publish: boolean) {
  if (saving.value) return
  error.value = ''
  notice.value = ''
  serverProblem.value = null
  const b = blocks.value
  if ((isNew.value && b.createBlocked) || (publish && b.publishBlocked)) { error.value = pt('errExamsDisabled'); return }
  if (!subjectId.value) { error.value = pt('exChooseSubject'); step.value = 1; return }
  if (!f.title.trim()) { error.value = pt('invalidTitle'); step.value = 3; return }
  if (!frozen.value && firstBad.value) { error.value = pt('exFixProblems'); step.value = 2; return }
  if (publish && !questions.value.length) { error.value = pt('errExQuestionCount'); step.value = 1; return }
  if (f.release === 'after_close' && !f.closes) { error.value = pt('exReleaseNeedsClose'); step.value = 3; return }
  saving.value = true
  try {
    let d: ExamDetail
    if (isNew.value) {
      d = await api.create({ ...payload(), status: publish ? 'published' : 'draft' })
      adopt(d) // marks the form clean (teacher) before the address changes: the guard must not ask about what was just saved
      await router.replace(`${listPath}/${d.id}/edit`)
    } else {
      d = await api.update(examId.value, payload())
      adopt(d)
      if (publish && d.status === 'draft') {
        d = await api.action(d.id, 'publish')
        adopt(d)
      }
    }
    notice.value = publish ? pt('exPublished') : pt('exSaved')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    if (e instanceof PlatformError) {
      if (e.code === 'exams_disabled') refreshStats() // the numbers were stale: show the notice
      if (typeof e.data.question_id === 'string') {
        serverProblem.value = { id: e.data.question_id, code: e.code }
        step.value = 2
      } else if (typeof e.data.index === 'number' && questions.value[e.data.index]) {
        serverProblem.value = { id: questions.value[e.data.index]!.id, code: e.code }
        step.value = 2
      }
      if (STALE.has(e.code) && examId.value) {
        try { exam.value = await api.get(examId.value) } catch { /* the message above stays */ }
      }
    }
  } finally {
    saving.value = false
  }
}

const goStep = (n: 1 | 2 | 3) => { step.value = n }
const probText = (p: Problem) => pt(`exProblem_${p}` as PlatformKey)
const stepLabel = (n: number) => pt((['exStep1', 'exStep2', 'exStep3'] as const)[n - 1]!)
const canPublish = computed(() => st.value.canPublish)
</script>

<template>
  <div class="max-w-4xl mx-auto pb-16 space-y-4" data-testid="exam-editor">
    <div class="flex items-center gap-3 flex-wrap">
      <router-link :to="listPath" class="btn-text" data-testid="back-to-list">← {{ isTeacher ? pt('mexTitle') : pt('exTitle') }}</router-link>
      <h1 class="text-display-sm font-bold tracking-tight flex-1 min-w-0 break-words">{{ loadFailed ? pt('loadErrorTitle') : isNew ? pt('exNew') : f.title }}</h1>
      <span v-if="exam" class="text-xs font-semibold px-2 py-1 rounded-full" style="background-color: rgb(var(--md-surface-container-high))" data-testid="exam-phase">{{ pt(`exPhase_${exam.phase}` as PlatformKey) }}</span>
    </div>

    <!-- the admin switched exam creation off (teachers only): say so before the form is filled in -->
    <p v-if="blocks.createBlocked" class="card-filled p-3 text-body-md" role="status" data-testid="exams-off-create">{{ pt('exDisabledCreate') }}</p>
    <p v-else-if="blocks.publishBlocked" class="card-filled p-3 text-body-md" role="status" data-testid="exams-off-publish">{{ pt('exDisabledPublish') }}</p>

    <p v-if="st.notice === 'teacher-owned'" class="card-filled p-3 text-body-md" role="status" data-testid="teacher-owned">{{ pt('exTeacherExam') }}</p>
    <p v-else-if="st.notice === 'locked'" class="card-filled p-3 text-body-md" role="status" data-testid="exam-locked">{{ pt('exLockedNotice') }}</p>
    <p v-else-if="st.notice === 'archived'" class="card-filled p-3 text-body-md" role="status" data-testid="exam-archived">{{ pt('exArchivedNotice') }}</p>
    <p v-else-if="st.notice === 'frozen'" class="card-filled p-3 text-body-md" role="status" data-testid="frozen-notice">{{ pt('exFrozenNotice') }}</p>

    <nav v-if="!loadFailed" class="flex gap-2 overflow-x-auto" :aria-label="pt('exTitle')">
      <button v-for="n in ([1, 2, 3] as const)" :key="n" :class="step === n ? 'btn-filled' : 'btn-outlined'" class="whitespace-nowrap" :aria-current="step === n ? 'step' : undefined" :data-testid="'step-' + n" @click="goStep(n)">
        <span dir="ltr" class="inline-block">{{ n }}.</span> {{ stepLabel(n) }}
      </button>
    </nav>

    <p v-if="notice" role="status" class="text-body-md" data-testid="ex-notice">{{ notice }}</p>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="ex-error">{{ error }}</p>

    <!-- ── step 1: source -->
    <section v-show="step === 1 && !loadFailed" class="space-y-4" data-testid="panel-1">
      <div v-if="isNew || !exam">
        <SubjectPicker v-if="!isTeacher" v-model="subjectId" />
        <SubjectPicker v-else-if="teachingLoaded && !routeId && !teachingFailed" v-model="subjectId" :remember="false" active-only :allowed-subject-ids="allowedIds" :subject-institutions="subjectInstitutions" />
        <p v-if="isTeacher && !routeId && teachingFailed" class="card-filled p-3 text-body-md" data-testid="editor-teaching-failed">
          <button type="button" class="btn-tonal" data-testid="editor-retry-teaching" @click="retryTeaching">{{ pt('exRetryTeaching') }}</button>
        </p>
        <p v-if="isTeacher && teachingLoaded && !routeId && !teachingFailed && isNew && !usable.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="editor-no-subjects">
          {{ pt('noApprovedSubjects') }} <router-link to="/platform/teaching" class="underline font-semibold" data-testid="editor-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
        </p>
      </div>
      <p v-else class="text-body-md"><b>{{ pt('bankSubject') }}:</b> {{ exam.subject_name }}</p>
      <p v-if="!subjectId" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="ex-choose">{{ pt('exChooseSubject') }}</p>
      <template v-else-if="!locked">
        <p class="card-elevated p-3 text-body-md" data-testid="ex-count">{{ pt('exInExam') }}: <b dir="ltr" class="inline-block" data-testid="ex-count-n">{{ questions.length }}</b></p>
        <p v-if="isTeacher" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="ex-teacher-sources">{{ pt('exTeacherSources') }}</p>
        <ExamAiPanel v-if="!isTeacher" :subject-id="subjectId" @generated="addGenerated" />
        <ExamImportPanel :existing="questions" :room="MAX_EXAM_QUESTIONS - questions.length" @imported="addImported" />
        <ExamBankPicker v-if="!isTeacher" :subject-id="subjectId" :taken="taken" @add="addFromBank" />
        <ExamLocalBankPicker v-else :existing="questions" @add="addImported" />
        <section class="card-filled p-5 space-y-3">
          <h2 class="text-title-md font-bold">{{ pt('exSourceManual') }}</h2>
          <button v-if="!showManual" class="btn-tonal" data-testid="manual-open" @click="showManual = true">{{ pt('exManualTitle') }}</button>
          <BankItemForm v-else hide-tags @save="addManual" @cancel="showManual = false" />
        </section>
      </template>
      <div class="flex justify-end"><button class="btn-filled" data-testid="next-1" @click="goStep(2)">{{ pt('exNext') }}</button></div>
    </section>

    <!-- ── step 2: review -->
    <section v-show="step === 2 && !loadFailed" class="space-y-3" data-testid="panel-2">
      <div class="card-elevated p-3 flex flex-wrap gap-4 text-body-md" data-testid="ex-totals">
        <span>{{ pt('exQuestionCount') }}: <b dir="ltr" class="inline-block" data-testid="tot-count">{{ questions.length }}</b></span>
        <span>{{ pt('exTotalPoints') }}: <b dir="ltr" class="inline-block" data-testid="tot-points">{{ sum.points }}</b></span>
        <span v-for="(n, t) in sum.byType" :key="t">{{ pt(`bank_t_${t}` as PlatformKey) }}: <b dir="ltr" class="inline-block">{{ n }}</b></span>
      </div>
      <p v-if="!questions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="review-empty">{{ pt('exReviewEmpty') }}</p>
      <ol class="space-y-3">
        <li v-for="(q, i) in questions" :key="q.id" class="card-filled p-4 space-y-2" :class="problems.get(q.id) || serverProblem?.id === q.id ? 'ring-2' : ''" :style="problems.get(q.id) || serverProblem?.id === q.id ? { '--tw-ring-color': 'rgb(var(--md-error))' } : {}" data-testid="review-q">
          <div class="flex items-start gap-3">
            <span dir="ltr" class="inline-block font-bold shrink-0">{{ i + 1 }}.</span>
            <div class="flex-1 min-w-0 space-y-2">
              <div class="flex flex-wrap gap-2 text-xs font-semibold">
                <span class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(`bank_t_${q.type}` as PlatformKey) }}</span>
                <span v-if="q.difficulty" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(`bank_d_${q.difficulty}` as PlatformKey) }}</span>
                <span v-if="q.chapter" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ q.chapter }}</span>
                <span v-if="q.src" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))">{{ pt('exSourceBank') }}</span>
              </div>
              <div class="whitespace-pre-wrap break-words font-semibold" dir="auto" data-testid="review-stem">{{ q.stem || '—' }}</div>
              <ol v-if="shownOptions(q).length" class="space-y-1">
                <li v-for="(o, k) in shownOptions(q)" :key="k" class="flex gap-2" :class="isCorrect(q, k) ? 'font-bold' : ''"><span dir="ltr" class="inline-block w-6 shrink-0">{{ letter(k) }}.</span><span class="flex-1 break-words" dir="auto">{{ o }}</span><span v-if="isCorrect(q, k)" aria-hidden="true">✓</span></li>
              </ol>
              <div v-else class="text-body-md" dir="auto"><span class="font-bold">✓</span> {{ q.answer }}</div>
              <p v-if="problems.get(q.id)" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))" data-testid="q-problem">{{ probText(problems.get(q.id) as Problem) }}</p>
              <p v-else-if="serverProblem?.id === q.id" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))" data-testid="q-problem-server">{{ error }}</p>
              <div class="flex flex-wrap items-center gap-2">
                <label class="flex items-center gap-2 text-body-sm">{{ pt('exScore') }}
                  <input :value="q.score ?? ''" type="number" min="0" max="100" step="0.5" class="input-outlined w-20 !py-1" dir="ltr" :disabled="locked" data-testid="q-score" @input="setScore(q, ($event.target as HTMLInputElement).value)" />
                </label>
                <template v-if="!locked">
                  <button class="btn-text" :disabled="i === 0" :aria-label="pt('exMoveUp')" data-testid="q-up" @click="shift(i, -1)">↑</button>
                  <button class="btn-text" :disabled="i === questions.length - 1" :aria-label="pt('exMoveDown')" data-testid="q-down" @click="shift(i, 1)">↓</button>
                  <button class="btn-text" data-testid="q-edit" @click="editingId = editingId === q.id ? '' : q.id">{{ pt('bankEdit') }}</button>
                  <button class="btn-text" data-testid="q-remove" @click="remove(q.id)">{{ pt('exRemove') }}</button>
                </template>
              </div>
            </div>
          </div>
          <BankItemForm v-if="editingId === q.id" hide-tags :item="asBankItem(q)" @save="saveEdit" @cancel="editingId = ''" />
        </li>
      </ol>
      <div class="flex justify-between"><button class="btn-outlined" data-testid="back-2" @click="goStep(1)">{{ pt('exBackStep') }}</button><button class="btn-filled" data-testid="next-2" @click="goStep(3)">{{ pt('exNext') }}</button></div>
    </section>

    <!-- ── step 3: settings & publish -->
    <section v-show="step === 3 && !loadFailed" class="space-y-4" data-testid="panel-3">
      <div class="card-filled p-5 space-y-4">
        <label class="block"><span class="text-label-lg">{{ pt('exFieldTitle') }}</span>
          <input v-model="f.title" maxlength="200" class="input-outlined w-full mt-1" :disabled="readOnly" data-testid="f-title" /></label>
        <label class="block"><span class="text-label-lg">{{ pt('exFieldDesc') }}</span>
          <textarea v-model="f.description" rows="3" maxlength="2000" dir="auto" class="input-outlined w-full mt-1" :disabled="readOnly" data-testid="f-description"></textarea></label>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <label class="block"><span class="text-label-lg">{{ pt('exDuration') }}</span>
            <input v-model.number="f.duration" type="number" min="1" max="480" dir="ltr" class="input-outlined w-full mt-1" :disabled="locked" data-testid="f-duration" /></label>
          <label class="block"><span class="text-label-lg">{{ pt('exAttemptsField') }}</span>
            <input v-model.number="f.attempts" type="number" min="1" max="10" dir="ltr" class="input-outlined w-full mt-1" :disabled="st.settingsLocked" data-testid="f-attempts" /></label>
          <label class="block"><span class="text-label-lg">{{ pt('exOpens') }}</span>
            <input v-model="f.opens" type="datetime-local" dir="ltr" class="input-outlined w-full mt-1" :disabled="locked" data-testid="f-opens" /></label>
          <label class="block"><span class="text-label-lg">{{ pt('exCloses') }}</span>
            <input v-model="f.closes" type="datetime-local" dir="ltr" class="input-outlined w-full mt-1" :disabled="st.settingsLocked" data-testid="f-closes" /></label>
        </div>
        <div class="space-y-2">
          <label class="flex items-start gap-3"><input v-model="f.shuffleQ" type="checkbox" class="mt-1 h-5 w-5" :disabled="locked" data-testid="f-shuffle-q" /><span>{{ pt('exShuffleQ') }}</span></label>
          <label class="flex items-start gap-3"><input v-model="f.shuffleO" type="checkbox" class="mt-1 h-5 w-5" :disabled="locked" data-testid="f-shuffle-o" /><span>{{ pt('exShuffleO') }}</span></label>
          <label class="flex items-start gap-3"><input v-model="f.showAnswers" type="checkbox" class="mt-1 h-5 w-5" :disabled="st.settingsLocked" data-testid="f-show-answers" /><span>{{ pt('exShowAnswers') }}</span></label>
        </div>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <label class="block"><span class="text-label-lg">{{ pt('exPassMark') }}</span>
            <input v-model.number="f.passMark" type="number" min="1" max="100" dir="ltr" class="input-outlined w-full mt-1" :disabled="st.settingsLocked" data-testid="f-pass" /></label>
          <label class="block"><span class="text-label-lg">{{ pt('exRelease') }}</span>
            <select v-model="f.release" class="input-outlined w-full mt-1" :disabled="st.settingsLocked" data-testid="f-release">
              <option value="immediate">{{ pt('exReleaseNow') }}</option><option value="after_close">{{ pt('exReleaseClose') }}</option>
            </select></label>
        </div>
        <p v-if="f.release === 'after_close' && !f.closes" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="release-warn">{{ pt('exReleaseNeedsClose') }}</p>
      </div>
      <div class="flex flex-wrap gap-2 items-center">
        <button class="btn-outlined" data-testid="open-preview" @click="showPreview = true">{{ pt('exPreview') }}</button>
        <template v-if="!readOnly && !st.archived">
          <button v-if="isNew || canPublish" class="btn-tonal" :disabled="saving || (isNew && blocks.createBlocked)" data-testid="save-draft" @click="save(false)">{{ isNew ? pt('exSaveDraft') : pt('exSaveChanges') }}</button>
          <button v-if="isNew || canPublish" class="btn-filled" :disabled="saving || blocks.publishBlocked" data-testid="publish" @click="save(true)">{{ pt('exPublishNow') }}</button>
          <button v-else class="btn-filled" :disabled="saving" data-testid="save-changes" @click="save(false)">{{ pt('exSaveChanges') }}</button>
        </template>
      </div>
      <div class="flex justify-start"><button class="btn-outlined" data-testid="back-3" @click="goStep(2)">{{ pt('exBackStep') }}</button></div>
    </section>

    <ExamPreview v-if="showPreview" :title="f.title" :description="f.description" :questions="questions" :duration-min="f.duration || null" :max-attempts="f.attempts" :shuffle="f.shuffleQ || f.shuffleO" @close="showPreview = false" />
    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
  </div>
</template>
