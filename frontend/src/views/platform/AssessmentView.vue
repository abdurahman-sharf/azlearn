<script setup lang="ts">
import { ref, computed, onMounted, onBeforeUnmount, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, platformAdminErrorMessage, type PlatformKey } from '@/i18n/platform'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getAssessment, type AssessmentDetail } from '@/api/platformExams'
import { adminExams, type ExamAction } from '@/api/platformExamAdmin'
import PageError from '@/components/platform/PageError.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import ConfirmDialog from '@/components/platform/ConfirmDialog.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import { PlatformError } from '@/lib/platformApi'
import { enroll } from '@/api/platformLearning'
import { countdown, openRecheck, reasonKey, stateChip } from '@/utils/studentExams'
import { useStudentStats } from '@/lib/studentStats'
import { examClock } from '@/utils/serverClock'
import { setStartFlag } from '@/utils/startFlag'
import { startPreview } from '@/utils/startPreview'
import { practiceAccess } from '@/utils/practiceGate'
import { fillTemplate } from '@/utils/notificationText'

const pt = usePt()
const auth = useAuthStore()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string

const a = ref<AssessmentDetail | null>(null)
const error = ref('')
const isOwner = computed(() => a.value?.teacher_id === auth.profile?.id)
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load() {
  try {
    const t0 = Date.now()
    const d = await getAssessment(id)
    // the page measures how far this device's clock is from the server's (round-trip corrected): the end time the start
    // dialog shows, and later the exam timer, follow the server's clock
    examClock.sync(d.server_now, t0, Date.now())
    a.value = d
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
// Students have started the exam: deleting it would erase their results, so the admin's delete asks for the exam's title
// (the server demands it too) and closing or archiving is the better choice. Also set when the server says so after the
// page was loaded (a student started in the meantime).
const started = ref(false)
const hasStarted = computed(() => started.value || (a.value?.attempt_count ?? 0) > 0)
const isAdmin = computed(() => auth.role === 'admin')
/** an exam the admin made himself (a teacher's is only moderated: closed, archived, reopened, deleted) */
const adminOwned = computed(() => !a.value?.teacher_id)
const editLink = computed(() => (isOwner.value ? `/platform/exams/${id}/edit` : isAdmin.value && adminOwned.value ? `/platform/admin/exams/${id}/edit` : ''))
const busy = ref(false)
/** The server says attempts exist (`has_attempts`, or `confirm_required` = the typed title is needed): the page was loaded before the first student started. */
const startedMeanwhile = (e: unknown) => e instanceof PlatformError && (e.code === 'has_attempts' || e.code === 'confirm_required')
function fail(e: unknown) {
  if (startedMeanwhile(e)) started.value = true
  error.value = platformAdminErrorMessage(pt, e)
}
/** The admin's lifecycle actions: the same server rules as the admin exams screen. */
async function act(action: ExamAction) {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    await adminExams.action(id, action)
    await load()
  } catch (e) {
    fail(e)
    await load()
  } finally {
    busy.value = false
  }
}
const removing = ref(false)
const removeError = ref('')
function askRemove() {
  removeError.value = ''
  removing.value = true
}
async function remove() {
  if (!a.value || busy.value) return
  busy.value = true
  removeError.value = ''
  try {
    await adminExams.remove(id, hasStarted.value ? a.value.title : undefined)
    removing.value = false
    await router.replace('/platform/admin/exams')
  } catch (e) {
    removeError.value = platformAdminErrorMessage(pt, e)
    if (startedMeanwhile(e)) {
      // a student started after this page loaded: the dialog now asks for the exam's title, with the real attempt count
      started.value = true
      await load()
    }
  } finally {
    busy.value = false
  }
}

// ── the student's view of the exam: its state for me, why I cannot start it, enrolling from here, and the time left
// until an upcoming exam opens (the page asks the server again when it does, and keeps asking until the server agrees)
const isStudent = computed(() => auth.role === 'student')
const running = computed(() => !!a.value?.in_progress_attempt || a.value?.state === 'in_progress')
/** the chip: "open but not enrolled" reads "Not enrolled", never "Missed" (the exam is still there to be taken) */
const chip = computed(() => stateChip(a.value?.state, a.value?.reason))
const stateLabel = computed(() => (chip.value ? pt(`stexState_${chip.value}` as PlatformKey) : ''))
const reasonText = computed(() => {
  const k = a.value?.state === 'in_progress' ? null : reasonKey(a.value?.reason)
  return k ? pt(k as PlatformKey) : ''
})
const { refreshAfterChange: refreshStudent } = useStudentStats()
const enrolling = ref(false)
async function enrollHere() {
  if (!a.value || enrolling.value) return
  enrolling.value = true
  error.value = ''
  try {
    await enroll(a.value.subject_id)
    await load()
    void refreshStudent() // the sidebar's "My exams" badge changes with the enrolment
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    enrolling.value = false
  }
}
// ── starting: a confirmation first (what the attempt will be and when it really ends), and a choice between a graded attempt
// and a practice one when the teacher allows practice. Nothing starts from here: the take page starts (or resumes) the attempt.
const starting = ref(false)
const startMode = ref<'graded' | 'practice'>('graded')
const previewNow = ref(Date.now())
const preview = computed(() =>
  startPreview({ now: previewNow.value, durationMin: a.value?.duration_min, closesAt: a.value?.closes_at, attemptsUsed: a.value?.attempts_used ?? 0, maxAttempts: a.value?.max_attempts ?? 1 }),
)
function openStart() {
  startMode.value = 'graded'
  previewNow.value = examClock.now()
  starting.value = true
}
function confirmStart() {
  starting.value = false
  if (startMode.value === 'practice' && practice.value === 'available') {
    router.push(`/platform/assessments/${id}/practice`)
    return
  }
  // the take page starts a NEW attempt only while this confirmation is fresh (a link, a reload or the back button never does)
  setStartFlag(id)
  router.push(`/platform/assessments/${id}/take`)
}
/**
 * What this student is offered about practice right now (the server decides for real): the link, a "later" hint, or nothing.
 * Never while their graded attempt is running (the key is not for the sitting they are in), nor before the exam opens, nor
 * while the results are withheld and not yet released.
 */
const practice = computed(() =>
  a.value && isStudent.value
    ? practiceAccess({
        allowPractice: a.value.allow_practice,
        reason: a.value.reason,
        state: running.value ? 'in_progress' : a.value.state,
        status: a.value.status,
        opensAt: a.value.opens_at,
        releaseMode: a.value.release_mode,
        closesAt: a.value.closes_at,
        now: examClock.now(),
      })
    : 'hidden',
)
// ── closing an exam is final for the attempts in progress: they are submitted from their last autosave at once and reopening
// does not bring them back, so it is confirmed first
const closing = ref(false)
function confirmClose() {
  closing.value = false
  void act('close')
}

const now = ref(Date.now())
let tick: ReturnType<typeof setInterval> | null = null
const waitingToOpen = computed(() => isStudent.value && a.value?.state === 'upcoming' && !!a.value.opens_at)
const opensIn = computed(() => (waitingToOpen.value && a.value?.opens_at ? countdown(a.value.opens_at, now.value) : null))
const two = (n: number) => String(n).padStart(2, '0')
// Once this device's clock reaches `opens_at` the page asks again, every few seconds while the server still says
// "upcoming" (its clock may be a little behind), then hands over to a button.
let tries = 0
let lastTryAt = 0
let asking = false
const gaveUp = ref(false)
async function askAgain() {
  if (asking) return
  asking = true
  tries++
  lastTryAt = Date.now()
  try {
    await load()
  } finally {
    asking = false
  }
}
watch(now, (n) => {
  if (!waitingToOpen.value || asking) return
  const next = openRecheck({ state: a.value?.state, opensAt: a.value?.opens_at, now: n, tries, lastTryAt })
  if (next === 'retry') void askAgain()
  else if (next === 'give_up') gaveUp.value = true
})
function checkAgain() {
  tries = 0
  gaveUp.value = false
  void askAgain()
}
watch(waitingToOpen, (on) => {
  if (on && !tick) tick = setInterval(() => { now.value = Date.now() }, 1000)
  if (!on && tick) { clearInterval(tick); tick = null }
  if (!on) { tries = 0; gaveUp.value = false }
}, { immediate: true })
onBeforeUnmount(() => { if (tick) clearInterval(tick) })

onMounted(load)
</script>

<template>
  <div v-if="a" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${a.subject_id}`" class="text-body-sm underline">{{ a.subject_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words" dir="auto">{{ a.title }}</h1>
    <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('by') }} {{ a.teacher_name }} · {{ a.question_count }} {{ pt('questionsCount') }} · {{ a.total_points }} {{ pt('points') }}
      <template v-if="a.duration_min"> · {{ a.duration_min }} {{ pt('minutesShort') }}</template>
      <template v-if="a.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <p v-if="a.description" class="text-body-lg mb-3 whitespace-pre-line" dir="auto">{{ a.description }}</p>
    <p v-if="a.opens_at || a.closes_at" class="text-body-sm mb-3">
      <template v-if="a.opens_at">{{ pt('opensAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.opens_at) }}</template>
      <template v-if="a.closes_at"> · {{ pt('closesAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.closes_at) }}</template>
    </p>
    <p v-if="a.pass_mark || a.release_mode === 'after_close'" class="text-body-sm mb-3" data-testid="exam-terms">
      <template v-if="a.pass_mark">{{ pt('tkPassMark') }}: <span dir="ltr" class="inline-block">{{ a.pass_mark }}%</span></template><template v-if="a.pass_mark && a.release_mode === 'after_close'"> · </template><template v-if="a.release_mode === 'after_close'">{{ pt('tkResultAfterClose') }}</template>
    </p>
    <p v-if="a.status === 'closed' || a.status === 'archived'" class="text-body-md font-semibold mb-3" data-testid="exam-closed">{{ pt('tkClosedExam') }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <template v-if="auth.role === 'student'">
      <div v-if="a.state" class="flex flex-wrap items-center gap-2 mb-3">
        <span class="text-sm font-semibold px-3 py-1 rounded-full" style="background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))" data-testid="exam-state">{{ stateLabel }}</span>
        <span v-if="reasonText" class="text-body-md" data-testid="exam-reason">{{ reasonText }}</span>
      </div>
      <p v-if="opensIn && !opensIn.done" class="text-body-md mb-3" role="timer" data-testid="exam-opens-in">
        {{ pt('stexOpensIn') }}:
        <template v-if="opensIn.days"><span dir="ltr" class="inline-block font-bold">{{ opensIn.days }}</span> {{ pt('stexDays') }} · </template>
        <span dir="ltr" class="inline-block font-bold tabular-nums">{{ two(opensIn.hours) }}:{{ two(opensIn.minutes) }}:{{ two(opensIn.seconds) }}</span>
      </p>
      <!-- the opening time has passed here but the server still says "upcoming": it keeps being asked; after a while a button takes over -->
      <p v-else-if="opensIn" class="text-body-md mb-3 flex flex-wrap items-center gap-2" role="status" data-testid="exam-opens-checking">
        <template v-if="!gaveUp">{{ pt('stexOpensChecking') }}</template>
        <button v-else type="button" class="btn-tonal" data-testid="exam-opens-refresh" @click="checkAgain">{{ pt('stexOpensRefresh') }}</button>
      </p>
      <div v-if="a.reason === 'not_enrolled'" class="card-filled p-4 mb-4 flex flex-wrap items-center gap-3">
        <p class="text-body-md flex-1 min-w-0">{{ pt('stexEnrollFirst') }}</p>
        <button type="button" class="btn-filled" :disabled="enrolling" data-testid="exam-enroll" @click="enrollHere">{{ pt('enroll') }}</button>
      </div>
      <section v-if="a.can_start && !a.in_progress_attempt" class="card-filled p-4 mb-4 space-y-1" data-testid="exam-rules">
        <h2 class="font-bold">{{ pt('tkRulesTitle') }}</h2>
        <ul class="list-disc ps-5 text-body-sm space-y-0.5"><li>{{ pt('tkRules1') }}</li><li>{{ pt('tkRules2') }}</li><li v-if="a.shuffle_questions || a.shuffle_options">{{ pt('tkRules3') }}</li><li>{{ pt('tkRules4') }}</li><li data-testid="exam-rules-hard-end">{{ pt('tkRulesHardEnd') }}</li></ul>
      </section>
      <p class="text-body-sm mb-3">{{ pt('attemptsUsed') }}: <span dir="ltr" class="inline-block">{{ a.attempts_used }} / {{ a.max_attempts }}</span></p>
      <!-- the server decides (can_start also covers resuming a running attempt); the label says which -->
      <router-link v-if="a.can_start && running" :to="`/platform/assessments/${a.id}/take`" class="btn-filled inline-flex mb-4" data-testid="exam-resume">{{ pt('resumeAssessment') }}</router-link>
      <button v-else-if="a.can_start" type="button" class="btn-filled inline-flex mb-4" data-testid="exam-start" @click="openStart">{{ pt('startAssessment') }}</button>
      <!-- practice: the questions with their answers, graded by nobody (available when the exam is closed or the attempts are used up too) -->
      <div v-if="practice !== 'hidden'" class="card-filled p-4 mb-4 flex flex-wrap items-center gap-3" data-testid="exam-practice-card">
        <p class="text-body-sm flex-1 min-w-0">{{ pt('examPracticeHint') }}</p>
        <router-link v-if="practice === 'available'" :to="`/platform/assessments/${a.id}/practice`" class="btn-tonal" data-testid="exam-practice">{{ pt('examPractice') }}</router-link>
        <p v-else class="text-body-sm w-full font-semibold" data-testid="exam-practice-later">{{ pt('prcNotAvailable') }}</p>
      </div>
      <ul class="space-y-2">
        <li v-for="t in a.attempts.filter(x => x.status !== 'in_progress')" :key="t.attempt_id">
          <router-link :to="`/platform/attempts/${t.attempt_id}`" class="card-filled block p-3">
            <span v-if="t.status === 'submitted' && !t.released" class="text-body-sm" data-testid="attempt-embargo">{{ pt('tkResultAfterClose') }}</span>
            <span v-else-if="t.status === 'submitted'" class="font-bold inline-block" dir="ltr">{{ t.score }} / {{ t.total }}</span>
            <span v-else>{{ pt('expiredAttempt') }}</span>
            <span v-if="t.pending" class="text-body-sm ms-2">· {{ pt('pendingGrading') }}</span>
            <span v-if="t.passed !== null" class="text-body-sm ms-2 font-semibold">· {{ t.passed ? pt('tkPassed') : pt('tkFailed') }}</span>
            <span class="text-body-sm ms-2" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(t.started_at) }}</span>
          </router-link>
        </li>
      </ul>
    </template>

    <div v-if="!isOwner" class="mt-4"><ReportButton target-type="assessment" :target-id="a.id" /></div>

    <div v-if="isOwner || isAdmin || (auth.role === 'teacher' && !a.teacher_id)" class="flex flex-wrap gap-2 mt-4">
      <router-link :to="`/platform/assessments/${a.id}/results`" class="btn-filled">{{ pt('results') }} ({{ a.attempt_count }})</router-link>
      <router-link v-if="editLink" :to="editLink" class="btn-outlined" data-testid="exam-edit">{{ pt('edit') }}</router-link>
      <router-link v-if="isOwner" to="/platform/exams" class="btn-text" data-testid="exam-open-mine">{{ pt('mexTitle') }}</router-link>
      <template v-if="isAdmin">
        <button v-if="a.status === 'published' && !hasStarted" class="btn-outlined" :disabled="busy" data-testid="exam-unpublish" @click="act('unpublish')">{{ pt('unpublish') }}</button>
        <button v-if="a.status === 'published'" class="btn-outlined" :disabled="busy" data-testid="exam-close" @click="closing = true">{{ pt('exClose') }}</button>
        <button v-if="a.status === 'closed'" class="btn-outlined" :disabled="busy" data-testid="exam-reopen" @click="act('reopen')">{{ pt('exReopen') }}</button>
        <button v-if="a.status === 'draft' || a.status === 'closed'" class="btn-outlined" :disabled="busy" data-testid="exam-archive" @click="act('archive')">{{ pt('exArchive') }}</button>
        <button v-if="a.status === 'archived'" class="btn-outlined" :disabled="busy" data-testid="exam-restore" @click="act('restore')">{{ pt('exRestore') }}</button>
        <button v-if="a.locked" class="btn-outlined" :disabled="busy" data-testid="exam-unlock" @click="act('unlock')">{{ pt('exUnlock') }}</button>
        <button class="btn-outlined" :disabled="busy" data-testid="exam-delete" @click="askRemove">{{ pt('del') }}</button>
        <router-link to="/platform/admin/exams" class="btn-text" data-testid="exam-open-admin">{{ pt('openAdminExams') }}</router-link>
      </template>
    </div>
    <p v-if="isAdmin && hasStarted" class="text-body-sm mt-2" data-testid="exam-admin-hint" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminExamsHint') }}</p>
    <ConfirmDialog
      v-if="starting"
      testid="start-confirm"
      :title="pt('scTitle')"
      :message="pt('scMessage')"
      :confirm-label="startMode === 'practice' ? pt('scConfirmPractice') : pt('scConfirmGraded')"
      @confirm="confirmStart"
      @close="starting = false"
    >
      <ul v-if="startMode === 'graded'" class="list-disc ps-5 text-body-md space-y-1" data-testid="start-confirm-details">
        <li data-testid="start-confirm-duration">
          {{ pt('scDuration') }}:
          <template v-if="preview.durationMin"><span dir="ltr" class="inline-block font-semibold">{{ preview.durationMin }}</span> {{ pt('minutesShort') }}</template>
          <template v-else>{{ pt('scOpenEnded') }}</template>
        </li>
        <li data-testid="start-confirm-end">
          <template v-if="preview.endsAt">{{ pt('scEnds') }}: <span class="font-semibold" dir="auto">{{ fmt(preview.endsAt) }}</span><template v-if="preview.limitedByClosing"> <span data-testid="start-confirm-limited">{{ pt('scEndsClosing') }}</span></template></template>
          <template v-else>{{ pt('scNoEnd') }}</template>
        </li>
        <li data-testid="start-confirm-attempt">{{ fillTemplate(pt('scAttemptOf'), { k: preview.attemptNo, n: preview.attemptsMax }) }}</li>
      </ul>
      <fieldset v-if="practice === 'available'" class="space-y-2" data-testid="start-mode">
        <legend class="font-semibold mb-1">{{ pt('scModeLegend') }}</legend>
        <label class="flex items-start gap-3 card-filled p-3 cursor-pointer">
          <input v-model="startMode" type="radio" name="start-mode" value="graded" class="mt-1 h-5 w-5" data-testid="start-mode-graded" />
          <span><span class="block font-semibold">{{ pt('scModeGraded') }}</span><span class="block text-body-sm">{{ pt('scModeGradedHint') }}</span></span>
        </label>
        <label class="flex items-start gap-3 card-filled p-3 cursor-pointer">
          <input v-model="startMode" type="radio" name="start-mode" value="practice" class="mt-1 h-5 w-5" data-testid="start-mode-practice" />
          <span><span class="block font-semibold">{{ pt('scModePractice') }}</span><span class="block text-body-sm">{{ pt('scModePracticeHint') }}</span></span>
        </label>
      </fieldset>
    </ConfirmDialog>
    <ConfirmDialog
      v-if="closing"
      testid="close-confirm"
      :title="pt('exCloseConfirmTitle')"
      :message="pt('exCloseConfirmMsg')"
      :confirm-label="pt('exCloseConfirmYes')"
      @confirm="confirmClose"
      @close="closing = false"
    />
    <ConfirmDeleteDialog
      v-if="removing"
      :title="`${pt('del')}: ${a.title}`"
      :message="hasStarted ? pt('exDeleteMsgAttempts') : pt('exDeleteMsg')"
      :counts="[{ label: pt('exAttemptsCount'), value: a.attempt_count }, { label: pt('questionsCount'), value: a.question_count }]"
      :confirm-name="hasStarted ? a.title : undefined"
      :busy="busy"
      :error="removeError"
      @confirm="remove"
      @close="removing = false"
    />
  </div>
  <PageError v-else-if="error" :message="error" />
  <PageLoading v-else />
</template>
