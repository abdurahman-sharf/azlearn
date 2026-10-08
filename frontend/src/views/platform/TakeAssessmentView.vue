<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { startAssessment, submitAttempt, saveAnswers, sendAttemptEvent, type StartRes } from '@/api/platformExams'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string

const session = ref<StartRes | null>(null)
const answers = ref<Record<string, string>>({})
const index = ref(0)
const error = ref('')
const submitting = ref(false)
const confirming = ref(false)
const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | null = null

// ── autosave: debounced after edits (and at most every 30 s while dirty), so the attempt resumes on any device
const saveState = ref<'idle' | 'saving' | 'saved' | 'failed'>('idle')
let dirty = false
let saveTimer: ReturnType<typeof setTimeout> | null = null
let sweep: ReturnType<typeof setInterval> | null = null
let saving = false

function scheduleSave(delay = 5000) {
  if (saveTimer || !session.value) return
  saveTimer = setTimeout(() => { saveTimer = null; void doSave() }, delay)
}
async function doSave() {
  if (!session.value || submitting.value || saving || !dirty) return
  saving = true
  dirty = false
  saveState.value = 'saving'
  try {
    await saveAnswers(session.value.attempt_id, answers.value)
    saveState.value = 'saved'
  } catch (e) {
    dirty = true
    saveState.value = 'failed'
    if (e instanceof PlatformError && (e.code === 'time_expired' || e.code === 'already_submitted')) {
      // the server already settled this attempt (time ran out): show what it recorded
      router.replace(`/platform/attempts/${session.value.attempt_id}`)
      return
    }
    scheduleSave(10_000)
  } finally {
    saving = false
  }
}

// ── integrity: count each time the student leaves the page/tab (informational only; nothing is blocked)
let away = false
function leave() {
  if (away || !session.value || submitting.value) return
  away = true
  void doSave() // flush the latest answers while the tab is hidden
  sendAttemptEvent(session.value.attempt_id, 'tab_leave').catch(() => { /* best effort */ })
}
const back = () => { away = false }
const onVisibility = () => (document.visibilityState === 'hidden' ? leave() : back())

const LABELS = 'ABCDEFGHIJ'.split('')
const storageKey = () => `exameow-attempt-${session.value?.attempt_id}`
const q = computed(() => session.value?.questions[index.value] ?? null)
const answered = computed(() => session.value?.questions.filter(x => (answers.value[x.id] ?? '').trim()).length ?? 0)
const remainingSec = computed(() => (session.value ? Math.max(0, Math.floor((session.value.ends_at - now.value) / 1000)) : 0))
const timeText = computed(() => `${String(Math.floor(remainingSec.value / 60)).padStart(2, '0')}:${String(remainingSec.value % 60).padStart(2, '0')}`)
const timed = computed(() => session.value ? session.value.ends_at - session.value.started_at < 24 * 3_600_000 : false)

onMounted(async () => {
  try {
    session.value = await startAssessment(id)
    // the server's autosave is the base; a newer local draft on this device wins per question
    answers.value = { ...session.value.saved_answers }
    try {
      const saved = localStorage.getItem(storageKey())
      if (saved) answers.value = { ...answers.value, ...JSON.parse(saved) }
    } catch { /* no saved draft */ }
    document.addEventListener('visibilitychange', onVisibility)
    window.addEventListener('blur', leave)
    window.addEventListener('focus', back)
    sweep = setInterval(() => { if (dirty) void doSave() }, 30_000)
    timer = setInterval(() => {
      now.value = Date.now()
      if (timed.value && remainingSec.value <= 0 && !submitting.value) submit()
    }, 1000)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
  if (sweep) clearInterval(sweep)
  if (saveTimer) clearTimeout(saveTimer)
  document.removeEventListener('visibilitychange', onVisibility)
  window.removeEventListener('blur', leave)
  window.removeEventListener('focus', back)
})

// Keep a local draft so a refresh does not lose answers (the server only stores them on submit).
watch(answers, (v) => {
  if (!session.value) return
  try { localStorage.setItem(storageKey(), JSON.stringify(v)) } catch { /* storage unavailable */ }
  dirty = true
  scheduleSave()
}, { deep: true })

const set = (qid: string, v: string) => { answers.value = { ...answers.value, [qid]: v } }
const isOn = (qid: string, label: string, multi: boolean) => (multi ? (answers.value[qid] ?? '').includes(label) : answers.value[qid] === label)
function pick(qid: string, label: string, multi: boolean) {
  if (!multi) return set(qid, label)
  const cur = new Set((answers.value[qid] ?? '').split(''))
  if (cur.has(label)) cur.delete(label)
  else cur.add(label)
  set(qid, [...cur].sort().join(''))
}

async function submit() {
  if (!session.value || submitting.value) return
  submitting.value = true
  error.value = ''
  try {
    const r = await submitAttempt(session.value.attempt_id, answers.value)
    try { localStorage.removeItem(storageKey()) } catch { /* ignore */ }
    router.replace(`/platform/attempts/${r.attempt_id}`)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    submitting.value = false
    confirming.value = false
  }
}
</script>

<template>
  <div v-if="session && q" class="max-w-2xl mx-auto pb-8">
    <div class="flex items-center gap-3 mb-3">
      <span class="text-body-sm flex-1">{{ pt('question') }} <span dir="ltr" class="inline-block">{{ index + 1 }} / {{ session.questions.length }}</span> · {{ pt('answered') }}: {{ answered }}</span>
      <span class="text-body-sm" data-testid="save-state" aria-live="polite">{{ saveState === 'saving' ? pt('tkSaving') : saveState === 'saved' ? pt('tkSaved') : saveState === 'failed' ? pt('tkSaveFailed') : '' }}</span>
      <span v-if="timed" class="font-bold tabular-nums" :style="{ color: remainingSec < 60 ? 'rgb(var(--md-error))' : undefined }" data-testid="timer" :aria-label="pt('timeLeft')">{{ timeText }}</span>
    </div>

    <div class="card-elevated p-5 space-y-4">
      <p class="text-body-lg whitespace-pre-wrap break-words" dir="auto" data-testid="stem">{{ q.stem }}</p>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ q.points }} {{ pt('points') }}</p>

      <div v-if="q.type === 'single_choice' || q.type === 'multi_choice'" class="space-y-2">
        <button v-for="(o, i) in q.options" :key="i" type="button" class="w-full text-start card-filled p-3" :class="{ 'ring-2': isOn(q.id, LABELS[i]!, q.type === 'multi_choice') }" :aria-pressed="isOn(q.id, LABELS[i]!, q.type === 'multi_choice')" @click="pick(q.id, LABELS[i]!, q.type === 'multi_choice')">
          <span dir="ltr" class="font-bold me-2 inline-block">{{ LABELS[i] }}.</span><span dir="auto">{{ o }}</span>
        </button>
      </div>
      <div v-else-if="q.type === 'true_false'" class="flex gap-2">
        <button type="button" class="flex-1" :class="answers[q.id] === 'A' ? 'btn-filled' : 'btn-outlined'" @click="set(q.id, 'A')">{{ pt('trueLabel') }}</button>
        <button type="button" class="flex-1" :class="answers[q.id] === 'B' ? 'btn-filled' : 'btn-outlined'" @click="set(q.id, 'B')">{{ pt('falseLabel') }}</button>
      </div>
      <input v-else-if="q.type === 'fill_blank'" :value="answers[q.id] ?? ''" maxlength="500" class="input-outlined w-full" @input="set(q.id, ($event.target as HTMLInputElement).value)" />
      <textarea v-else :value="answers[q.id] ?? ''" maxlength="2000" rows="5" class="input-outlined w-full" @input="set(q.id, ($event.target as HTMLTextAreaElement).value)"></textarea>
    </div>

    <div class="flex items-center gap-2 mt-4">
      <button class="btn-outlined" :disabled="index === 0" @click="index--">{{ pt('previous') }}</button>
      <button class="btn-outlined" :disabled="index >= session.questions.length - 1" @click="index++">{{ pt('next') }}</button>
      <button class="btn-filled ms-auto" @click="confirming = true">{{ pt('submitAttempt') }}</button>
    </div>

    <div class="flex flex-wrap gap-1 mt-4" role="navigation">
      <button v-for="(x, i) in session.questions" :key="x.id" class="w-9 h-9 rounded-full text-sm" :class="{ 'ring-2': i === index }" :style="{ backgroundColor: (answers[x.id] ?? '').trim() ? 'rgb(var(--md-primary-container))' : 'rgb(var(--md-surface-container-high))' }" @click="index = i">{{ i + 1 }}</button>
    </div>

    <div v-if="confirming" class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.4)" role="dialog" aria-modal="true">
      <div class="card-elevated p-5 max-w-sm w-full space-y-3">
        <p>{{ pt('confirmSubmit') }}</p>
        <p v-if="answered < session.questions.length" class="text-body-sm">{{ pt('unansweredWarning') }} {{ session.questions.length - answered }}</p>
        <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
        <div class="flex gap-2">
          <button class="btn-filled" :disabled="submitting" data-testid="confirm-submit" @click="submit">{{ pt('submitAttempt') }}</button>
          <button class="btn-outlined" @click="confirming = false">{{ pt('cancel') }}</button>
        </div>
      </div>
    </div>
  </div>
  <div v-else class="max-w-2xl mx-auto">
    <p v-if="error" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <router-link :to="`/platform/assessments/${id}`" class="underline">{{ pt('back') }}</router-link>
  </div>
</template>
