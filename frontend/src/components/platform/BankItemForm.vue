<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { BANK_DIFFICULTIES, BANK_TYPES, type BankItem, type BankItemInput, type BankType } from '@/api/platformBank'

const props = defineProps<{ item?: BankItem | null }>()
const emit = defineEmits<{ save: [BankItemInput]; cancel: [] }>()
const pt = usePt()
const letter = (i: number) => String.fromCharCode(65 + i)

const f = reactive({
  type: 'single_choice' as BankType,
  stem: '',
  options: ['', '', '', ''],
  correct: new Set<number>(),
  tf: 'A' as 'A' | 'B',
  text: '',
  analysis: '',
  chapter: '',
  difficulty: '' as '' | 'easy' | 'medium' | 'hard',
  tags: '',
})
const problem = ref(false)

function load(i: BankItem | null | undefined) {
  problem.value = false
  f.type = i?.type ?? 'single_choice'
  f.stem = i?.stem ?? ''
  f.options = i && (i.type === 'single_choice' || i.type === 'multi_choice') ? [...i.options] : ['', '', '', '']
  f.correct = new Set(i && (i.type === 'single_choice' || i.type === 'multi_choice') ? [...i.answer].map((c) => c.charCodeAt(0) - 65) : [])
  f.tf = i?.type === 'true_false' && i.answer === 'B' ? 'B' : 'A'
  f.text = i && (i.type === 'fill_blank' || i.type === 'short_answer') ? i.answer : ''
  f.analysis = i?.analysis ?? ''
  f.chapter = i?.chapter ?? ''
  f.difficulty = i?.difficulty ?? ''
  f.tags = (i?.tags ?? []).join(', ')
}
watch(() => props.item, load, { immediate: true })

const isChoice = computed(() => f.type === 'single_choice' || f.type === 'multi_choice')
const typeLabel = (t: BankType) => pt(`bank_t_${t}` as PlatformKey)

function toggle(i: number) {
  if (f.type === 'single_choice') f.correct = new Set([i])
  else if (f.correct.has(i)) f.correct.delete(i)
  else f.correct.add(i)
}
function addOption() { if (f.options.length < 10) f.options.push('') }
function removeOption(i: number) {
  if (f.options.length <= 2) return
  f.options.splice(i, 1)
  f.correct = new Set([...f.correct].filter((c) => c !== i).map((c) => (c > i ? c - 1 : c)))
}
function onType() {
  // a single-choice question keeps at most one marked answer
  if (f.type === 'single_choice' && f.correct.size > 1) f.correct = new Set([[...f.correct][0]!])
}

function submit() {
  const base = { analysis: f.analysis.trim(), chapter: f.chapter.trim() || null, difficulty: f.difficulty || null, tags: f.tags.split(/[,،]/).map((t) => t.trim()).filter(Boolean) }
  const stem = f.stem.trim()
  if (!stem) { problem.value = true; return }
  if (isChoice.value) {
    // Empty option boxes are dropped, so the correct letters are re-derived from the kept positions.
    const kept = f.options.map((text, idx) => ({ text: text.trim(), idx })).filter((o) => o.text)
    const letters = kept.map((o, pos) => (f.correct.has(o.idx) ? letter(pos) : '')).join('')
    if (kept.length < 2 || !letters) { problem.value = true; return }
    emit('save', { type: f.type, stem, options: kept.map((o) => o.text), answer: letters, ...base })
  } else if (f.type === 'true_false') {
    emit('save', { type: f.type, stem, options: [], answer: f.tf, ...base })
  } else {
    if (!f.text.trim()) { problem.value = true; return }
    emit('save', { type: f.type, stem, options: [], answer: f.text.trim(), ...base })
  }
}
</script>

<template>
  <form class="card-filled p-5 space-y-4" data-testid="bank-form" @submit.prevent="submit">
    <label class="block">
      <span class="text-label-lg">{{ pt('bankFormType') }}</span>
      <select v-model="f.type" class="input-outlined w-full mt-1" data-testid="form-type" @change="onType">
        <option v-for="t in BANK_TYPES" :key="t" :value="t">{{ typeLabel(t) }}</option>
      </select>
    </label>
    <label class="block">
      <span class="text-label-lg">{{ pt('bankFormStem') }}</span>
      <textarea v-model="f.stem" rows="3" maxlength="3000" dir="auto" class="input-outlined w-full mt-1" data-testid="form-stem"></textarea>
    </label>

    <fieldset v-if="isChoice" class="space-y-2">
      <legend class="text-label-lg">{{ pt('bankFormOptions') }} / {{ pt('bankFormAnswer') }}</legend>
      <div v-for="(_, i) in f.options" :key="i" class="flex items-center gap-2">
        <input
          :type="f.type === 'single_choice' ? 'radio' : 'checkbox'"
          :checked="f.correct.has(i)"
          :aria-label="pt('bankFormAnswer') + ' ' + letter(i)"
          class="h-5 w-5 shrink-0"
          :data-testid="'form-correct-' + i"
          @change="toggle(i)"
        />
        <span dir="ltr" class="inline-block w-6 shrink-0 font-bold">{{ letter(i) }}.</span>
        <input v-model="f.options[i]" maxlength="1000" dir="auto" class="input-outlined flex-1 min-w-0" :data-testid="'form-option-' + i" />
        <button v-if="f.options.length > 2" type="button" class="btn-text" :aria-label="pt('bankFormRemoveOption')" @click="removeOption(i)">✕</button>
      </div>
      <button v-if="f.options.length < 10" type="button" class="btn-text" data-testid="form-add-option" @click="addOption">+ {{ pt('bankFormAddOption') }}</button>
    </fieldset>

    <fieldset v-else-if="f.type === 'true_false'" class="flex gap-4">
      <legend class="text-label-lg mb-1">{{ pt('bankFormAnswer') }}</legend>
      <label class="flex items-center gap-2"><input v-model="f.tf" type="radio" value="A" data-testid="form-tf-true" /> {{ pt('bankFormTrue') }}</label>
      <label class="flex items-center gap-2"><input v-model="f.tf" type="radio" value="B" data-testid="form-tf-false" /> {{ pt('bankFormFalse') }}</label>
    </fieldset>

    <label v-else class="block">
      <span class="text-label-lg">{{ pt('bankFormAnswer') }}</span>
      <textarea v-model="f.text" :rows="f.type === 'short_answer' ? 3 : 1" maxlength="2000" dir="auto" class="input-outlined w-full mt-1" data-testid="form-text"></textarea>
      <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ f.type === 'fill_blank' ? pt('bankFormAnswerFillHint') : pt('bankFormAnswerShortHint') }}</span>
    </label>

    <label class="block">
      <span class="text-label-lg">{{ pt('bankFormAnalysis') }}</span>
      <textarea v-model="f.analysis" rows="2" maxlength="6000" dir="auto" class="input-outlined w-full mt-1" data-testid="form-analysis"></textarea>
    </label>
    <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
      <label class="block"><span class="text-label-lg">{{ pt('bankFormChapter') }}</span>
        <input v-model="f.chapter" maxlength="100" class="input-outlined w-full mt-1" data-testid="form-chapter" /></label>
      <label class="block"><span class="text-label-lg">{{ pt('bankFormDifficulty') }}</span>
        <select v-model="f.difficulty" class="input-outlined w-full mt-1" data-testid="form-difficulty">
          <option value="">{{ pt('bankNoDifficulty') }}</option>
          <option v-for="d in BANK_DIFFICULTIES" :key="d" :value="d">{{ pt(`bank_d_${d}` as PlatformKey) }}</option>
        </select></label>
      <label class="block"><span class="text-label-lg">{{ pt('bankFormTags') }}</span>
        <input v-model="f.tags" class="input-outlined w-full mt-1" data-testid="form-tags" /></label>
    </div>
    <p v-if="problem" role="alert" class="text-body-sm" style="color: rgb(var(--md-error))" data-testid="form-problem">{{ pt('errBankQuestion') }}</p>
    <div class="flex gap-2">
      <button class="btn-filled" data-testid="form-save">{{ pt('bankSave') }}</button>
      <button type="button" class="btn-outlined" data-testid="form-cancel" @click="emit('cancel')">{{ pt('bankCancel') }}</button>
    </div>
  </form>
</template>
