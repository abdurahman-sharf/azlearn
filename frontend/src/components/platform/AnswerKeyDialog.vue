<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import type { Question } from '@exameow/shared'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'
import { examApiFor, type CorrectResult } from '@/api/platformExamAdmin'
import { letter } from '@/utils/examBuilder'
import { answerIsPicked, buildKeyBody, indexesToLetters, initialForm, isVoided, lettersToIndexes, type KeyForm, type KeyProblem } from '@/utils/answerKey'

// Corrects one question's answer key after students have answered (the owner of the exam only; the server refuses anyone
// else). The teacher changes the correct answer, the points or the explanation - or voids the question - and sees what
// that would do (a dry run: how many attempts and students are touched, how many scores go up or down) before applying.
// Applying re-grades the submitted attempts of this one question in one transaction; the points a grader gave to written
// answers are kept. Preview first and the button turns into "confirm and apply"; without a preview the first press
// previews and the second applies.
const props = defineProps<{ examId: string; questionId: string }>()
const emit = defineEmits<{ close: []; applied: [CorrectResult] }>()
const pt = usePt()
const auth = useAuthStore()
const api = examApiFor(auth.role)

const open = ref(true)
const panel = ref<HTMLElement | null>(null)
// Closing while the correction is being written would drop the `applied` event (the emit of an unmounted component is
// ignored) and leave the results page showing the old numbers: Escape and Cancel wait until the request is back.
function close() {
  if (!busy.value) emit('close')
}
useDialog(open, panel, close)

const question = ref<Question | null>(null)
const loading = ref(true)
const busy = ref(false)
const previewing = ref(false)
const error = ref('')
const preview = ref<{ key: string; result: CorrectResult } | null>(null)
const form = reactive<KeyForm>({ mode: 'set', answer: '', score: '', analysis: '' })

const picked = computed(() => !!question.value && answerIsPicked(question.value.type))
const optionList = computed(() => {
  const q = question.value
  if (!q) return []
  return q.type === 'true_false' && !q.options.length ? ['صحيح', 'خطأ'] : q.options
})
const alreadyVoided = computed(() => !!question.value && isVoided(question.value))
const chosenIndexes = computed(() => new Set(lettersToIndexes(form.answer)))

const built = computed(() => (question.value ? buildKeyBody(question.value, form) : null))
const bodyKey = computed(() => (built.value?.body ? JSON.stringify(built.value.body) : ''))
const fresh = computed(() => !!preview.value && preview.value.key === bodyKey.value)
const PROBLEM: Record<KeyProblem, PlatformKey> = { nothing: 'keyNothing', answer: 'errBankAnswer', score: 'invalidPoints' }
const problem = computed(() => (built.value?.problem ? pt(PROBLEM[built.value.problem]) : ''))
const canRun = computed(() => !!built.value?.body && !busy.value && !previewing.value)

onMounted(async () => {
  try {
    const exam = await api.get(props.examId)
    const q = exam.questions.find((x) => x.id === props.questionId)
    if (!q) {
      error.value = pt('notFound')
      return
    }
    question.value = q
    Object.assign(form, initialForm(q))
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
})

// Any change to the form invalidates the numbers shown; they are recalculated on the next preview.
watch(bodyKey, () => { error.value = '' })

function toggleChoice(i: number) {
  const q = question.value
  if (!q) return
  if (q.type === 'multi_choice') {
    const next = new Set(chosenIndexes.value)
    if (!next.delete(i)) next.add(i)
    form.answer = indexesToLetters(next)
  } else {
    form.answer = letter(i)
  }
}

async function run(dry: boolean): Promise<CorrectResult | null> {
  const b = built.value?.body
  if (!b) return null
  error.value = ''
  if (dry) previewing.value = true
  else busy.value = true
  try {
    return await api.correctQuestion(props.examId, props.questionId, { ...b, dry_run: dry })
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    return null
  } finally {
    previewing.value = false
    busy.value = false
  }
}

async function doPreview() {
  if (!canRun.value) return
  const key = bodyKey.value
  const result = await run(true)
  if (result) preview.value = { key, result }
}

async function apply() {
  if (!canRun.value) return
  if (!fresh.value) {
    await doPreview() // the first press shows what would change; the same button then asks to confirm
    return
  }
  const result = await run(false)
  if (result) emit('applied', result)
}

const r = computed(() => (fresh.value ? preview.value!.result : null))
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center p-3 sm:p-4" style="background: rgb(0 0 0 / 0.5)" data-testid="key-dialog">
    <div ref="panel" class="card-elevated p-4 sm:p-5 w-full max-w-lg max-h-[92vh] overflow-y-auto space-y-4" role="dialog" aria-modal="true" aria-labelledby="key-dialog-title">
      <h2 id="key-dialog-title" class="text-title-lg font-bold">{{ pt('keyTitle') }}</h2>

      <p v-if="loading" role="status" class="text-body-md" data-testid="key-loading">{{ pt('keyLoading') }}</p>

      <template v-if="question">
        <div class="rounded-xl p-3" style="background-color: rgb(var(--md-surface-container-high))">
          <div class="whitespace-pre-wrap break-words font-semibold" dir="auto" data-testid="key-stem">{{ question.stem }}</div>
        </div>
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('keyIntro') }}</p>
        <p v-if="alreadyVoided" class="text-body-sm font-semibold" data-testid="key-voided-note">{{ pt('keyVoidedNote') }}</p>

        <fieldset class="space-y-2">
          <legend class="text-label-lg font-semibold">{{ pt('keyMode') }}</legend>
          <label class="flex items-start gap-3"><input v-model="form.mode" type="radio" name="key-mode" value="set" class="mt-1 h-5 w-5" data-autofocus data-testid="key-mode-set" /><span>{{ pt('keyModeSet') }}</span></label>
          <label class="flex items-start gap-3"><input v-model="form.mode" type="radio" name="key-mode" value="void" class="mt-1 h-5 w-5" :disabled="alreadyVoided" data-testid="key-mode-void" /><span>{{ pt('keyModeVoid') }}</span></label>
        </fieldset>

        <p v-if="form.mode === 'void'" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="key-void-hint">{{ pt('keyVoidHint') }}</p>

        <div v-else class="space-y-3">
          <!-- the correct answer: picked from the options for choice and true/false questions, typed for the others -->
          <fieldset v-if="picked" class="space-y-1" data-testid="key-answer">
            <legend class="text-label-lg font-semibold">{{ pt('keyAnswer') }}</legend>
            <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('keyAnswerChoiceHint') }}</p>
            <label v-for="(o, i) in optionList" :key="i" class="flex items-start gap-3">
              <input
                :type="question.type === 'multi_choice' ? 'checkbox' : 'radio'" name="key-answer" :value="letter(i)" class="mt-1 h-5 w-5 shrink-0"
                :checked="chosenIndexes.has(i)" :data-testid="`key-answer-${letter(i)}`" @change="toggleChoice(i)"
              />
              <span class="min-w-0"><span dir="ltr" class="inline-block font-bold">{{ letter(i) }}.</span> <span class="break-words" dir="auto">{{ o }}</span></span>
            </label>
          </fieldset>
          <label v-else-if="question.type === 'fill_blank'" class="block">
            <span class="text-label-lg font-semibold">{{ pt('keyAnswer') }}</span>
            <input v-model="form.answer" maxlength="2000" dir="auto" class="input-outlined w-full mt-1" data-testid="key-answer" />
            <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt('keyAnswerFillHint') }}</span>
          </label>
          <label v-else class="block">
            <span class="text-label-lg font-semibold">{{ pt('keyAnswer') }}</span>
            <textarea v-model="form.answer" rows="3" maxlength="2000" dir="auto" class="input-outlined w-full mt-1" data-testid="key-answer"></textarea>
            <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt('keyAnswerShortHint') }}</span>
          </label>

          <label class="block">
            <span class="text-label-lg font-semibold">{{ pt('keyScore') }}</span>
            <!-- not v-model: on a number input Vue would store a real number here, while the form (and its checks) work on the typed text -->
            <input :value="form.score" type="number" min="0" max="100" step="0.5" dir="ltr" class="input-outlined w-32 mt-1 block" data-testid="key-score" @input="form.score = ($event.target as HTMLInputElement).value" />
            <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt('keyScoreHint') }}</span>
          </label>
          <label class="block">
            <span class="text-label-lg font-semibold">{{ pt('keyAnalysis') }}</span>
            <textarea v-model="form.analysis" rows="3" maxlength="6000" dir="auto" class="input-outlined w-full mt-1" data-testid="key-analysis"></textarea>
          </label>
        </div>

        <p v-if="problem && form.mode === 'set'" class="text-body-sm" role="status" style="color: rgb(var(--md-on-surface-variant))" data-testid="key-problem">{{ problem }}</p>

        <!-- what the correction would change (a dry run: nothing is written) -->
        <section v-if="r" class="rounded-xl p-3 space-y-1" style="background-color: rgb(var(--md-surface-container-high))" aria-live="polite" data-testid="key-affected">
          <h3 class="text-title-md font-bold">{{ pt('keyEffect') }}</h3>
          <p v-if="!r.attempts_total" class="text-body-md" data-testid="key-none">{{ pt('keyNoAttempts') }}</p>
          <ul v-else class="text-body-md space-y-0.5">
            <li><b dir="ltr" class="inline-block" data-testid="key-n-attempts">{{ r.affected_attempts }}</b> {{ pt('keyAffectedAttempts') }}</li>
            <li><b dir="ltr" class="inline-block" data-testid="key-n-students">{{ r.affected_students }}</b> {{ pt('keyAffectedStudents') }}</li>
            <li><b dir="ltr" class="inline-block" data-testid="key-n-up">{{ r.up }}</b> {{ pt('keyUp') }} · <b dir="ltr" class="inline-block" data-testid="key-n-down">{{ r.down }}</b> {{ pt('keyDown') }} · <b dir="ltr" class="inline-block" data-testid="key-n-same">{{ r.unchanged }}</b> {{ pt('keyUnchanged') }}</li>
            <li><b dir="ltr" class="inline-block" data-testid="key-n-checked">{{ r.attempts_total }}</b> {{ pt('keyAttemptsTotal') }}</li>
          </ul>
          <p class="text-body-md">{{ pt('keyTotalChange') }}: <b dir="ltr" class="inline-block" data-testid="key-total">{{ r.old_total }} → {{ r.new_total }}</b></p>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="key-confirm-note">{{ pt('keyConfirmNote') }}</p>
        </section>
      </template>

      <p v-if="error" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))" data-testid="key-error">{{ error }}</p>

      <div class="flex flex-wrap gap-2 justify-end">
        <button type="button" class="btn-outlined" :disabled="busy" data-testid="key-cancel" @click="close">{{ pt('cancel') }}</button>
        <button v-if="question" type="button" class="btn-tonal" :disabled="!canRun" data-testid="key-preview" @click="doPreview">{{ previewing ? pt('keyPreviewing') : pt('keyPreview') }}</button>
        <button v-if="question" type="button" class="btn-filled" :disabled="!canRun" data-testid="key-apply" @click="apply">{{ busy ? pt('keyApplying') : fresh ? pt('keyApplyConfirm') : pt('keyApply') }}</button>
      </div>
    </div>
  </div>
</template>
