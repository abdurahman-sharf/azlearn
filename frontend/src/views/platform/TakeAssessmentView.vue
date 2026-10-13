<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { useDialog } from '@/composables/useDialog'
import ConfirmDialog from '@/components/platform/ConfirmDialog.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import { fillTemplate } from '@/utils/notificationText'
import { flushAnswers, getAssessment, saveAnswers, sendAttemptEvent, startAssessment, submitAttempt, type PublicQuestion, type StartRes } from '@/api/platformExams'
import { attemptDraftKey, attemptMarksKey, autoSubmitDue, isAttemptOverCode, parseDraft } from '@/utils/attemptDrafts'
import { examClock, clockText, secondsLeft } from '@/utils/serverClock'
import { clearStartFlag, readStartFlag, setStartFlag } from '@/utils/startFlag'
import {
  announceDue, answeredCount, applySaved, changedAnswers, clockStepped, fitsKeepalive, isAnswered, isRetryableSubmitError, isTimedAttempt, radioMove, retryDelayMs, seedAnnounced,
  shouldAskBeforeLeaving,
} from '@/utils/takeExam'
import { markedIndexes, navState, parseMarked, pruneMarked, serializeMarked, toggleMarked, unansweredIndexes } from '@/utils/reviewMarks'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const auth = useAuthStore()
const i18n = useI18nStore()
const id = route.params.id as string
const examPage = `/platform/assessments/${id}`
const rtl = computed(() => i18n.locale === 'ar')

const title = ref('')
const session = ref<StartRes | null>(null)
const answers = ref<Record<string, string>>({})
/** What the server is known to hold (its autosave, plus every save that went through): the autosave sends only the difference. */
const saved = ref<Record<string, string>>({})
const marked = ref<Set<string>>(new Set())
const index = ref(0)
/** The attempt could not be opened (before any question is shown). */
const loadError = ref('')
const loadRetryable = ref(false)
/** The last hand-in failed (shown on the page, outside the confirmation dialog). */
const submitError = ref('')
const submitting = ref(false)
const confirming = ref(false)
const dialogEl = ref<HTMLElement | null>(null)
useDialog(confirming, dialogEl, () => { confirming.value = false })
/** The server's clock, as this device sees it (updated every second from the measured offset). */
const now = ref(examClock.now())
const announce = ref('')
const stemEl = ref<HTMLElement | null>(null)
/** The answer options of the question on screen (one of the three groups below is rendered at a time). */
const optionsEl = ref<HTMLElement | null>(null)

const LABELS = 'ABCDEFGHIJ'.split('')
const storageKey = () => attemptDraftKey(session.value?.attempt_id ?? '')
const marksKey = () => attemptMarksKey(session.value?.attempt_id ?? '')
/** The server has already ended this attempt (submitted elsewhere, or settled when its time ran out). */
const attemptOver = (e: unknown) => e instanceof PlatformError && isAttemptOverCode(e.code)
/** Set once the server says the attempt is over (or it was handed in): no local copy is written any more, and nothing is sent. */
let over = false
/** Leaving the page on purpose (after the hand-in, or when the server ended the attempt): the leave confirmation stays out of the way. */
let allowLeave = false
/** When the page last tried to hand the attempt in by itself (the time ran out). */
let lastAutoTry = 0
/** A definite refusal (not a lost connection) ended the automatic tries: the person can still press "Submit" / "Try again now". */
let autoGaveUp = false

const q = computed<PublicQuestion | null>(() => session.value?.questions[index.value] ?? null)
const answered = computed(() => (session.value ? answeredCount(session.value.questions, answers.value) : 0))
const remainingSec = computed(() => (session.value ? secondsLeft(session.value.ends_at, now.value) : 0))
const timeText = computed(() => clockText(remainingSec.value))
const timed = computed(() => (session.value ? isTimedAttempt(session.value) : false))
const unsentAnswers = computed(() => changedAnswers(answers.value, saved.value))
const unsent = computed(() => Object.keys(unsentAnswers.value).length > 0)
const unansweredIdx = computed(() => (session.value ? unansweredIndexes(session.value.questions, answers.value) : []))
const markedIdx = computed(() => (session.value ? markedIndexes(session.value.questions, marked.value) : []))
const isMarked = computed(() => !!q.value && marked.value.has(q.value.id))

// ── autosave: debounced after edits (and at most every 30 s while something is unsent), so the attempt resumes on any device.
// Only the questions changed since the last good save are sent; the server merges them, so a second device saving other
// questions is never overwritten. Every reply also carries the server's clock and the attempt's CURRENT end, which the page
// adopts (the owner may have moved the closing time).
const saveState = ref<'idle' | 'saving' | 'saved' | 'failed'>('idle')
let saveTimer: ReturnType<typeof setTimeout> | null = null
let sweep: ReturnType<typeof setInterval> | null = null
/** The request on its way, if any: a second caller waits for its answer instead of sending the same keys again. */
let inflight: Promise<boolean> | null = null

function scheduleSave(delay = 5000) {
  if (saveTimer || !session.value) return
  saveTimer = setTimeout(() => { saveTimer = null; void doSave() }, delay)
}

/**
 * Takes the end the server derives now. It moves when the closing time is extended, so a page that is already open keeps
 * the time the owner gave (and the announcements and the "time is up" check start over for the longer time).
 */
function adoptEnd(endsAt: number | undefined, limited: boolean | undefined) {
  const s = session.value
  if (!s || typeof endsAt !== 'number' || !Number.isFinite(endsAt) || endsAt === s.ends_at) return
  session.value = { ...s, ends_at: endsAt, limited_by_closing: limited ?? s.limited_by_closing }
  now.value = examClock.now()
  if (secondsLeft(endsAt, now.value) > 0) {
    announced = seedAnnounced(secondsLeft(endsAt, now.value))
    timeUpSaid = false
    endConfirmed = false
  }
}

/** Sends the unsent answers. `force` sends even an empty list: a way to ask the server for its clock and the attempt's end. Resolves to whether the server took it. */
function doSave(force = false): Promise<boolean> {
  if (!session.value || submitting.value || over) return Promise.resolve(false)
  if (inflight) return inflight
  const delta = unsentAnswers.value
  const sending = Object.keys(delta).length > 0
  if (!force && !sending) return Promise.resolve(false)
  const attemptId = session.value.attempt_id
  const run = async (): Promise<boolean> => {
    if (sending) saveState.value = 'saving'
    const t0 = Date.now()
    try {
      const r = await saveAnswers(attemptId, delta)
      examClock.sync(r.server_now, t0, Date.now())
      adoptEnd(r.ends_at, r.limited_by_closing)
      saved.value = applySaved(saved.value, delta)
      if (sending) saveState.value = 'saved'
      if (unsent.value) scheduleSave() // edits made while the request was on its way
      return true
    } catch (e) {
      if (sending) saveState.value = 'failed'
      if (attemptOver(e)) {
        // the server already settled this attempt (time ran out, or it was handed in elsewhere): the local copy is of no use
        // any more; show what it recorded
        settledByServer()
        return false
      }
      if (unsent.value) scheduleSave(10_000)
      return false
    } finally {
      inflight = null
    }
  }
  inflight = run()
  return inflight
}

/** Best-effort save as the page is being left: the browser finishes a `keepalive` request after the page is gone. */
function flushUnsent() {
  if (!session.value || over || submitting.value) return
  const delta = unsentAnswers.value
  if (!Object.keys(delta).length) return
  if (!fitsKeepalive(JSON.stringify({ answers: delta }))) {
    void doSave()
    return
  }
  void flushAnswers(session.value.attempt_id, delta)
}

// ── integrity: count each time the student leaves the page/tab (informational only; nothing is blocked)
let away = false
function leave() {
  if (away || !session.value || submitting.value || over) return
  away = true
  flushUnsent() // keep the latest answers while the tab is hidden
  sendAttemptEvent(session.value.attempt_id, 'tab_leave').catch(() => { /* best effort */ })
}
const back = () => { away = false }
/** When the tab went into the background; a long absence (the device may have slept, the clock been corrected) is followed by a check with the server. */
let hiddenAt = 0
const LONG_ABSENCE_MS = 30_000
const onVisibility = () => {
  if (document.visibilityState === 'hidden') {
    hiddenAt = Date.now()
    leave()
    return
  }
  back()
  if (hiddenAt && Date.now() - hiddenAt > LONG_ABSENCE_MS && session.value && !over) void recheckEnd()
  hiddenAt = 0
}
const onBeforeUnload = (e: BeforeUnloadEvent) => {
  if (!session.value || over || allowLeave || !unsent.value) return
  e.preventDefault()
  e.returnValue = '' // some browsers only show their own prompt when this is set
}
const onPageHide = () => flushUnsent()

/** Forgets this attempt's local copy of the answers and marks (they are kept only while the attempt runs). */
function dropDraft() {
  if (!session.value) return
  try {
    localStorage.removeItem(storageKey())
    localStorage.removeItem(marksKey())
  } catch { /* storage unavailable */ }
}
/** Moves on to the result page; the leave confirmation must not stop this navigation. */
function goToResult(attemptId: string) {
  over = true
  allowLeave = true
  clearRetry()
  dropDraft()
  confirming.value = false
  router.replace(`/platform/attempts/${attemptId}`)
}
function settledByServer() {
  if (session.value) goToResult(session.value.attempt_id)
}

// ── opening the attempt. A NEW attempt starts only from a fresh confirmation (the exam page's dialog leaves a flag); a
// running attempt resumes without one; anything else goes back to the exam page.
function adopt(res: StartRes) {
  session.value = res
  saved.value = { ...res.saved_answers }
  // The server's autosave is the base. The local draft holds only what was edited ON THIS DEVICE and not yet sent (an empty
  // string = cleared here), and wins for those questions; every other question shows what the server holds, even when
  // another device changed it meanwhile.
  let draft: Record<string, string> = {}
  let marks = new Set<string>()
  try {
    draft = parseDraft(localStorage.getItem(attemptDraftKey(res.attempt_id)))
    marks = parseMarked(localStorage.getItem(attemptMarksKey(res.attempt_id)))
  } catch { /* no saved draft */ }
  answers.value = { ...res.saved_answers, ...draft }
  marked.value = pruneMarked(marks, res.questions)
  now.value = examClock.now()
}

async function begin() {
  loadError.value = ''
  loadRetryable.value = false
  const flag = readStartFlag(id)
  const t0 = Date.now()
  // the exam's title and the server's clock, and (without a confirmation) whether an attempt is already running
  const detail = getAssessment(id).then((d) => {
    examClock.sync(d.server_now, t0, Date.now())
    title.value = d.title
    return d
  })
  detail.catch(() => { /* the title is a nicety; the start below reports real problems */ })
  if (!flag.valid) {
    try {
      const d = await detail
      if (!d.in_progress_attempt && d.state !== 'in_progress') {
        router.replace(examPage)
        return
      }
    } catch (e) {
      // An exam students can no longer open (hidden or archived since) answers 404 / 403 here, but an attempt that is already
      // running may still be resumed - and the server never STARTS an attempt on an exam students cannot see - so the start
      // call below decides. Any other failure (no connection, server error) is shown with a retry.
      const hidden = e instanceof PlatformError && (e.code === 'not_found' || e.code === 'forbidden')
      if (!hidden) {
        loadError.value = platformErrorMessage(pt, e)
        loadRetryable.value = e instanceof PlatformError && e.code === 'network'
        return
      }
    }
  } else {
    clearStartFlag(id)
  }
  try {
    const t1 = Date.now()
    const res = await startAssessment(id)
    examClock.sync(res.server_now, t1, Date.now())
    adopt(res)
    startTimers()
  } catch (e) {
    // a dropped connection: the confirmation is still good for a retry
    if (flag.valid && flag.stamp !== null && e instanceof PlatformError && e.code === 'network') setStartFlag(id, flag.stamp)
    loadError.value = platformErrorMessage(pt, e)
    loadRetryable.value = e instanceof PlatformError && e.code === 'network'
  }
}

// ── the clock: the countdown, the announcements and the automatic hand-in all follow the server's time
let timer: ReturnType<typeof setInterval> | null = null
/** The announcements already made, or behind the page when it first read the time (seeded on the first tick; see `seedAnnounced`). */
let announced: Set<number> | null = null
let timeUpSaid = false
/** The server has been asked about the end after the time seemed up (or the device clock stepped): only then does the page hand in by itself. */
let endConfirmed = false
let endChecking = false
let lastWall = Date.now()
let lastMono = performance.now()
function startTimers() {
  document.addEventListener('visibilitychange', onVisibility)
  window.addEventListener('blur', leave)
  window.addEventListener('focus', back)
  window.addEventListener('beforeunload', onBeforeUnload)
  window.addEventListener('pagehide', onPageHide)
  sweep = setInterval(() => { if (unsent.value) void doSave() }, 30_000)
  lastWall = Date.now()
  lastMono = performance.now()
  timer = setInterval(tick, 1000)
  tick()
}

/**
 * Asks the server for its clock and the attempt's current end (an autosave reply carries both) and adopts them. The page
 * never ends an attempt on the strength of the DEVICE clock alone: a clock corrected by minutes mid-exam, or an extension of
 * the closing time the page has not heard of, would otherwise submit it early and for good.
 */
async function recheckEnd() {
  if (endChecking || !session.value || over) return
  endChecking = true
  try {
    await doSave(true)
  } finally {
    endChecking = false
    // Whether or not the server answered: if the time is still up by the (now corrected) clock and the (now current) end, handing
    // in is all that is left - and when the server could not be asked, handing in retries with a back-off. If time remains
    // (the closing time was extended, or the device clock was wrong), the next "time is up" is checked afresh.
    const s = session.value
    endConfirmed = !!s && secondsLeft(s.ends_at, examClock.now()) <= 0
  }
}

function tick() {
  const wall = Date.now()
  const mono = performance.now()
  const stepped = clockStepped(wall - lastWall, mono - lastMono)
  lastWall = wall
  lastMono = mono
  now.value = examClock.now()
  retryTick.value = wall
  if (!session.value || over) return
  if (stepped) {
    // the device clock moved (set by hand, corrected, or the device slept): what it shows is not to be trusted until the server confirms
    endConfirmed = false
    void recheckEnd()
  }
  if (announced === null) announced = seedAnnounced(remainingSec.value)
  if (timed.value) {
    const r = announceDue(remainingSec.value, announced)
    announced = r.announced
    if (r.minutes !== null) announce.value = pt(`tkxLeft_${r.minutes}` as PlatformKey)
    if (remainingSec.value <= 0 && !timeUpSaid) {
      timeUpSaid = true
      announce.value = pt('tkTimeUp')
    }
  }
  // time is up by the clock on screen: first ask the server (the end may have been moved, or the device clock be wrong)
  if (timed.value && remainingSec.value <= 0 && !endConfirmed) {
    void recheckEnd()
    return
  }
  // then hand the attempt in; a failed try (offline, server error) waits before the next, and once the server says
  // the attempt is over there are no more requests
  if (autoSubmitDue({ timed: timed.value, remainingSec: remainingSec.value, submitting: submitting.value || retryAt.value > 0, over: over || autoGaveUp, now: Date.now(), lastTryAt: lastAutoTry })) {
    lastAutoTry = Date.now()
    void submit()
  }
}

onMounted(() => { void begin() })
onUnmounted(() => {
  if (timer) clearInterval(timer)
  if (sweep) clearInterval(sweep)
  if (saveTimer) clearTimeout(saveTimer)
  clearRetry()
  removeGuard()
  if (pendingLeave) answerLeave(false)
  document.removeEventListener('visibilitychange', onVisibility)
  window.removeEventListener('blur', leave)
  window.removeEventListener('focus', back)
  window.removeEventListener('beforeunload', onBeforeUnload)
  window.removeEventListener('pagehide', onPageHide)
})

// Keep a local draft of what was edited here and not yet sent, so a refresh does not lose it; unsent answers go to the server soon.
watch(answers, () => {
  if (!session.value || over) return
  if (unsent.value) scheduleSave()
}, { deep: true })
watch(unsentAnswers, (v) => {
  if (!session.value || over) return
  try { localStorage.setItem(storageKey(), JSON.stringify(v)) } catch { /* storage unavailable */ }
}, { deep: true })
watch(marked, (v) => {
  if (!session.value || over) return
  try { localStorage.setItem(marksKey(), serializeMarked(v)) } catch { /* storage unavailable */ }
})

// ── answering
const set = (qid: string, v: string) => { answers.value = { ...answers.value, [qid]: v } }
const isOn = (qid: string, label: string, multi: boolean) => (multi ? (answers.value[qid] ?? '').includes(label) : answers.value[qid] === label)
function pick(qid: string, label: string, multi: boolean) {
  if (!multi) return set(qid, label)
  const cur = new Set((answers.value[qid] ?? '').split(''))
  if (cur.has(label)) cur.delete(label)
  else cur.add(label)
  set(qid, [...cur].sort().join(''))
}
const isChoice = (t: string) => t === 'single_choice' || t === 'multi_choice' || t === 'true_false'
/** Clears the answer; the button that did it disappears, so focus goes to the first option of the same question (not to the page's top). */
function clearAnswer(qid: string) {
  set(qid, '')
  void nextTick(() => optionsEl.value?.querySelector<HTMLElement>('button')?.focus())
}

/** One tab stop for a radio group: the chosen option, or the first one while nothing is chosen. */
function radioTab(qid: string, i: number, count: number): 0 | -1 {
  const cur = answers.value[qid]
  const chosen = cur ? LABELS.indexOf(cur) : -1
  return (chosen < 0 || chosen >= count ? i === 0 : chosen === i) ? 0 : -1
}
function onRadioKey(e: KeyboardEvent, qid: string, count: number) {
  if (e.altKey || e.ctrlKey || e.metaKey) return
  const target = (e.target as HTMLElement).closest<HTMLElement>('[role="radio"]')
  if (!target) return
  const next = radioMove(e.key, Number(target.dataset.index), count, rtl.value)
  if (next === null) return
  e.preventDefault()
  set(qid, LABELS[next]!)
  target.parentElement?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus()
}

// ── moving around: focus follows to the question's text, so a screen reader reads the new question
function focusStem() { void nextTick(() => stemEl.value?.focus()) }
function goTo(i: number) {
  if (!session.value) return
  index.value = Math.min(Math.max(0, i), session.value.questions.length - 1)
  focusStem()
}
function toggleMark() {
  if (q.value) marked.value = toggleMarked(marked.value, q.value.id)
}
function navLabel(i: number): string {
  const x = session.value?.questions[i]
  if (!x) return ''
  const parts = [`${pt('question')} ${i + 1}`, navState(x.id, answers.value) === 'answered' ? pt('answered') : pt('tkxNotAnswered')]
  if (marked.value.has(x.id)) parts.push(pt('tkxMarked'))
  return parts.join(rtl.value ? '، ' : ', ')
}
function jumpFromDialog(i: number) {
  confirming.value = false
  goTo(i)
}

// ── handing in. A failed try (no connection, a server hiccup) is shown on the page and tried again by itself after
// 2, 4, 8, 16, then 30 seconds; the person can also press "Try again now". The attempt ending on the server's side
// (time_expired / already_submitted) is not an error: the page moves on to the result.
const failures = ref(0)
const retryAt = ref(0)
const retryTick = ref(Date.now())
let retryTimer: ReturnType<typeof setTimeout> | null = null
const retryIn = computed(() => Math.max(0, Math.ceil((retryAt.value - retryTick.value) / 1000)))
function clearRetry() {
  if (retryTimer) clearTimeout(retryTimer)
  retryTimer = null
  retryAt.value = 0
}
function scheduleRetry() {
  failures.value++
  const delay = retryDelayMs(failures.value)
  clearRetry()
  retryTick.value = Date.now()
  retryAt.value = Date.now() + delay
  retryTimer = setTimeout(() => { retryTimer = null; retryAt.value = 0; void submit() }, delay)
}
function retryNow() {
  clearRetry()
  autoGaveUp = false
  submitError.value = '' // a manual try: a repeated failure is announced again
  void submit()
}

async function submit() {
  if (!session.value || submitting.value || over) return
  submitting.value = true
  // (the previous error stays on screen while an automatic retry is on its way: a repeated, identical failure is not announced again)
  clearRetry()
  try {
    // Only what changed on this page: the server lays it over what it already holds (its autosave, with the other device's
    // answers), so a page that is behind cannot erase them. An answer cleared here travels as an empty string.
    const r = await submitAttempt(session.value.attempt_id, unsentAnswers.value)
    goToResult(r.attempt_id)
  } catch (e) {
    if (attemptOver(e)) {
      // The server already ended this attempt (the device slept through the deadline, or it was handed in from another
      // tab or device) and settled it from the last autosave: leave for the page that shows what it recorded. Staying
      // here would be a dead end (focus mode has no navigation) with the same request failing again every second.
      settledByServer()
      return
    }
    submitError.value = `${pt('tkxSubmitFailed')} ${platformErrorMessage(pt, e)}`
    submitting.value = false
    confirming.value = false
    if (e instanceof PlatformError && isRetryableSubmitError(e)) scheduleRetry()
    else autoGaveUp = true
  }
}
function askToSubmit() {
  submitError.value = ''
  autoGaveUp = false
  confirming.value = true
}

// ── leaving the page while the attempt is open: the router asks first (the attempt and its timer keep running either way)
const leaveAsking = ref(false)
let pendingLeave: ((leave: boolean) => void) | null = null
function answerLeave(leaveNow: boolean) {
  leaveAsking.value = false
  const p = pendingLeave
  pendingLeave = null
  if (leaveNow) flushUnsent()
  p?.(leaveNow)
}
const removeGuard = router.beforeEach((to, from) => {
  if (!shouldAskBeforeLeaving({ active: !!session.value && !over && !allowLeave, loggedIn: auth.isLoggedIn, toPath: to.path, fromPath: from.path })) return true
  // a second navigation while a question is open cancels the first one
  pendingLeave?.(false)
  leaveAsking.value = true
  return new Promise<boolean>((resolve) => { pendingLeave = resolve })
})
</script>

<template>
  <div v-if="session && q" class="max-w-2xl mx-auto pb-8">
    <h1 class="text-title-lg font-bold break-words mb-2" dir="auto" data-testid="take-title">{{ title || pt('tkPageTitle') }}</h1>
    <!-- a failed hand-in (the confirmation dialog has closed): say so here, the answers are still on screen and can be sent again -->
    <div v-if="submitError && !confirming" class="mb-3 space-y-2" data-testid="take-submit-problem">
      <p class="text-body-sm font-semibold" role="alert" style="color: rgb(var(--md-error))" data-testid="take-error">{{ submitError }}</p>
      <!-- the countdown ticks every second and is only shown; the screen reader hears ONE "trying again" per scheduled retry -->
      <p v-if="retryAt > 0" class="text-body-sm" data-testid="take-retry-status">
        <span class="sr-only" role="status">{{ pt('tkxRetryingSoon') }}</span>
        <span aria-hidden="true">{{ fillTemplate(pt('tkxRetrying'), { s: retryIn, n: failures }) }}</span>
      </p>
      <button type="button" class="btn-tonal" :disabled="submitting" data-testid="take-retry" @click="retryNow">{{ pt('tkxRetryNow') }}</button>
    </div>
    <p v-if="submitting" class="sr-only" role="status">{{ pt('tkxSubmitting') }}</p>

    <!-- the progress row stays in view under the header (on a phone too) so the time left is always readable -->
    <div class="take-sticky sticky z-20 -mx-3 sm:mx-0 px-3 sm:px-0 py-2 mb-2" style="background-color: rgb(var(--md-surface))" data-testid="take-sticky">
      <div class="flex items-center gap-3 flex-wrap">
        <span class="text-body-sm flex-1 min-w-0">{{ pt('question') }} <span dir="ltr" class="inline-block">{{ index + 1 }} / {{ session.questions.length }}</span> · {{ pt('answered') }}: <span dir="ltr" class="inline-block">{{ answered }}</span></span>
        <span class="text-body-sm" data-testid="save-state" aria-live="polite">{{ saveState === 'saving' ? pt('tkSaving') : saveState === 'saved' ? pt('tkSaved') : saveState === 'failed' ? pt('tkSaveFailed') : '' }}</span>
        <span v-if="timed" class="font-bold tabular-nums text-body-lg" dir="ltr" :style="{ color: remainingSec < 60 ? 'rgb(var(--md-error))' : undefined }" data-testid="timer" role="timer" :aria-label="pt('timeLeft')">{{ timeText }}</span>
      </div>
      <p v-if="timed && session.limited_by_closing" class="text-body-sm mt-1" data-testid="take-limited">{{ pt('tkxLimitedByClosing') }}</p>
    </div>
    <!-- 10 / 5 / 1 minutes left, each said once to a screen reader -->
    <p class="sr-only" role="status" aria-live="polite" aria-atomic="true" data-testid="take-time-announce">{{ announce }}</p>

    <div class="card-elevated p-5 space-y-4">
      <p id="tk-stem" ref="stemEl" tabindex="-1" class="text-body-lg whitespace-pre-wrap break-words focus:outline-none scroll-mt-32" dir="auto" data-testid="stem">{{ q.stem }}</p>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ q.points }} {{ pt('points') }}</p>
      <p v-if="isMarked" class="text-body-sm font-semibold" data-testid="take-marked-current"><span aria-hidden="true">⚑</span> {{ pt('tkxMarked') }}</p>

      <div v-if="q.type === 'single_choice'" ref="optionsEl" class="space-y-2" role="radiogroup" aria-labelledby="tk-stem" data-testid="take-options" @keydown="onRadioKey($event, q.id, q.options.length)">
        <button
          v-for="(o, i) in q.options" :key="i" type="button" role="radio" class="w-full text-start card-filled p-3 flex items-start gap-2"
          :class="{ 'ring-2': isOn(q.id, LABELS[i]!, false) }" :aria-checked="isOn(q.id, LABELS[i]!, false)" :tabindex="radioTab(q.id, i, q.options.length)" :data-index="i"
          :data-testid="`take-option-${LABELS[i]}`" @click="pick(q.id, LABELS[i]!, false)"
        >
          <span dir="ltr" class="font-bold inline-block shrink-0">{{ LABELS[i] }}.</span><span dir="auto" class="flex-1 min-w-0 break-words">{{ o }}</span><span v-if="isOn(q.id, LABELS[i]!, false)" aria-hidden="true" class="shrink-0 font-bold">✓</span>
        </button>
      </div>
      <div v-else-if="q.type === 'multi_choice'" ref="optionsEl" class="space-y-2" role="group" aria-labelledby="tk-stem" data-testid="take-options">
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('tkxMultiHint') }}</p>
        <button
          v-for="(o, i) in q.options" :key="i" type="button" class="w-full text-start card-filled p-3 flex items-start gap-2" :class="{ 'ring-2': isOn(q.id, LABELS[i]!, true) }"
          :aria-pressed="isOn(q.id, LABELS[i]!, true)" :data-testid="`take-option-${LABELS[i]}`" @click="pick(q.id, LABELS[i]!, true)"
        >
          <span dir="ltr" class="font-bold inline-block shrink-0">{{ LABELS[i] }}.</span><span dir="auto" class="flex-1 min-w-0 break-words">{{ o }}</span><span v-if="isOn(q.id, LABELS[i]!, true)" aria-hidden="true" class="shrink-0 font-bold">✓</span>
        </button>
      </div>
      <div v-else-if="q.type === 'true_false'" ref="optionsEl" class="flex gap-2" role="group" aria-labelledby="tk-stem" data-testid="take-options">
        <button type="button" class="flex-1" :class="answers[q.id] === 'A' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="answers[q.id] === 'A'" data-testid="take-option-A" @click="set(q.id, 'A')">{{ pt('trueLabel') }}</button>
        <button type="button" class="flex-1" :class="answers[q.id] === 'B' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="answers[q.id] === 'B'" data-testid="take-option-B" @click="set(q.id, 'B')">{{ pt('falseLabel') }}</button>
      </div>
      <input v-else-if="q.type === 'fill_blank'" aria-labelledby="tk-stem" :value="answers[q.id] ?? ''" maxlength="500" dir="auto" class="input-outlined w-full" data-testid="take-input" @input="set(q.id, ($event.target as HTMLInputElement).value)" />
      <textarea v-else aria-labelledby="tk-stem" :value="answers[q.id] ?? ''" maxlength="2000" rows="5" dir="auto" class="input-outlined w-full" data-testid="take-input" @input="set(q.id, ($event.target as HTMLTextAreaElement).value)"></textarea>

      <div class="flex flex-wrap items-center gap-2">
        <button type="button" class="btn-text" :aria-pressed="isMarked" data-testid="take-review-toggle" @click="toggleMark"><span aria-hidden="true">⚑</span> {{ pt('tkxMark') }}</button>
        <button v-if="isChoice(q.type) && isAnswered(answers[q.id])" type="button" class="btn-text" data-testid="take-clear" @click="clearAnswer(q.id)">{{ pt('tkxClear') }}</button>
      </div>
    </div>

    <div class="flex items-center gap-2 mt-4">
      <button class="btn-outlined" :disabled="index === 0" data-testid="take-prev" @click="goTo(index - 1)">{{ pt('previous') }}</button>
      <button class="btn-outlined" :disabled="index >= session.questions.length - 1" data-testid="take-next" @click="goTo(index + 1)">{{ pt('next') }}</button>
      <button class="btn-filled ms-auto" data-testid="take-submit" @click="askToSubmit">{{ pt('submitAttempt') }}</button>
    </div>

    <!-- the navigator says it in words and shapes, not only in colour: a check = answered, a dashed outline = not answered, a flag = marked -->
    <nav class="flex flex-wrap gap-2 mt-4" :aria-label="pt('tkQuestionNav')" data-testid="take-nav">
      <button
        v-for="(x, i) in session.questions" :key="x.id" type="button" class="relative w-10 h-10 rounded-full text-sm font-semibold border-2"
        :class="[i === index ? 'ring-2 ring-offset-1' : '', navState(x.id, answers) === 'answered' ? 'border-solid' : 'border-dashed']"
        :style="{ backgroundColor: navState(x.id, answers) === 'answered' ? 'rgb(var(--md-primary-container))' : 'rgb(var(--md-surface-container-high))', color: 'rgb(var(--md-on-surface))', borderColor: 'rgb(var(--md-outline))' }"
        :aria-label="navLabel(i)" :aria-current="i === index ? 'step' : undefined" :data-testid="`take-nav-${i}`" @click="goTo(i)"
      >
        <span dir="ltr">{{ i + 1 }}</span>
        <span v-if="navState(x.id, answers) === 'answered'" aria-hidden="true" class="absolute -top-1 -end-1 text-[10px] leading-none rounded-full px-0.5" style="background-color: rgb(var(--md-surface)); color: rgb(var(--azl-success-text, var(--md-primary)))">✓</span>
        <span v-if="marked.has(x.id)" aria-hidden="true" :data-testid="`take-marked-${i}`" class="absolute -bottom-1 -end-1 text-[10px] leading-none rounded-full px-0.5" style="background-color: rgb(var(--md-surface)); color: rgb(var(--md-on-surface))">⚑</span>
      </button>
    </nav>

    <!-- the hand-in confirmation lists what is still open, each as a button that jumps to the question -->
    <div v-if="confirming" class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.4)" role="dialog" aria-modal="true" aria-labelledby="tk-confirm-msg">
      <div ref="dialogEl" class="card-elevated p-5 max-w-sm w-full max-h-[90vh] overflow-y-auto space-y-3">
        <p id="tk-confirm-msg">{{ pt('confirmSubmit') }}</p>
        <div v-if="unansweredIdx.length" class="space-y-1" data-testid="submit-unanswered">
          <p class="text-body-sm font-semibold">{{ pt('unansweredWarning') }} <span dir="ltr" class="inline-block">{{ unansweredIdx.length }}</span></p>
          <p class="text-body-sm">{{ pt('tkxUnansweredList') }}</p>
          <ul class="flex flex-wrap gap-1">
            <li v-for="i in unansweredIdx" :key="i">
              <button type="button" class="btn-outlined !px-3 !py-1 min-w-[2.5rem]" :aria-label="`${pt('tkxGoToQuestion')} ${i + 1}`" :data-testid="`submit-unanswered-${i}`" @click="jumpFromDialog(i)"><span dir="ltr">{{ i + 1 }}</span></button>
            </li>
          </ul>
        </div>
        <div v-if="markedIdx.length" class="space-y-1" data-testid="submit-marked">
          <p class="text-body-sm">{{ pt('tkxMarkedList') }}</p>
          <ul class="flex flex-wrap gap-1">
            <li v-for="i in markedIdx" :key="i">
              <button type="button" class="btn-outlined !px-3 !py-1 min-w-[2.5rem]" :aria-label="`${pt('tkxGoToQuestion')} ${i + 1}`" :data-testid="`submit-marked-${i}`" @click="jumpFromDialog(i)"><span aria-hidden="true">⚑</span> <span dir="ltr">{{ i + 1 }}</span></button>
            </li>
          </ul>
        </div>
        <div class="flex gap-2">
          <button class="btn-filled" :disabled="submitting" data-testid="confirm-submit" data-autofocus @click="submit">{{ submitting ? pt('tkxSubmitting') : pt('submitAttempt') }}</button>
          <button class="btn-outlined" @click="confirming = false">{{ pt('cancel') }}</button>
        </div>
      </div>
    </div>

    <ConfirmDialog
      v-if="leaveAsking"
      testid="take-leave-confirm"
      :title="pt('tkxLeaveTitle')"
      :message="pt('tkxLeaveMsg')"
      :confirm-label="pt('tkxLeaveYes')"
      :cancel-label="pt('tkxLeaveNo')"
      @confirm="answerLeave(true)"
      @close="answerLeave(false)"
    >
      <p v-if="unsent" class="text-body-sm" data-testid="take-leave-unsent">{{ pt('tkxLeaveUnsent') }}</p>
    </ConfirmDialog>
  </div>
  <div v-else-if="loadError" class="max-w-2xl mx-auto space-y-3">
    <h1 class="sr-only">{{ pt('tkPageTitle') }}</h1>
    <p role="alert" style="color: rgb(var(--md-error))" data-testid="take-load-error">{{ loadError }}</p>
    <div class="flex flex-wrap items-center gap-3">
      <button v-if="loadRetryable" type="button" class="btn-filled" data-testid="take-retry-load" @click="begin">{{ pt('offlineRetry') }}</button>
      <router-link :to="examPage" class="underline">{{ pt('back') }}</router-link>
    </div>
  </div>
  <PageLoading v-else label="tkxPreparing" />
</template>

<style scoped>
/* The progress row sticks just under the shell's header: the header's height (3.5rem, 4rem from the sm breakpoint) plus what sits
   above it (the desktop title bar, or the phone's safe-area inset), which AppShell publishes as --shell-top. */
.take-sticky {
  top: calc(var(--shell-top, 0px) + 3.5rem);
}
@media (min-width: 640px) {
  .take-sticky {
    top: calc(var(--shell-top, 0px) + 4rem);
  }
}
</style>
