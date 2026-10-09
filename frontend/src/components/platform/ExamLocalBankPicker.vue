<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { Question } from '@exameow/shared'
import { usePracticeStore } from '@/stores/practice'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { MAX_EXAM_QUESTIONS } from '@/utils/examImport'
import { letter } from '@/utils/examBuilder'
import { capState, chosenQuestions, filterLocal, selectAllWithin, selectNoneOf, takenMask } from '@/utils/localBankPicker'

// A teacher's way to reuse questions from the question banks kept in THIS browser (the generator's banks): pick a bank,
// search it, tick only the questions wanted and add those. Nothing is uploaded or changed in the bank; the ticked
// questions are copied (the editor gives them fresh ids and checks them like any other question). Questions whose text
// is already in the exam cannot be ticked again, and the exam's question limit is respected.
const props = defineProps<{ existing: Question[] }>()
const emit = defineEmits<{ add: [Question[]] }>()
const pt = usePt()
const practice = usePracticeStore()
const SHOW_STEP = 50

const bankId = ref(practice.banks[0]?.id ?? '')
const search = ref('')
const selected = ref<Set<number>>(new Set())
const shownCount = ref(SHOW_STEP)
const capped = ref(false)
const notice = ref('')

const banks = computed(() => practice.banks)
const bank = computed(() => banks.value.find((b) => b.id === bankId.value) ?? null)
const questions = computed<Question[]>(() => bank.value?.questions ?? [])
const taken = computed(() => takenMask(questions.value, props.existing))
const visible = computed(() => filterLocal(questions.value, search.value))
const shown = computed(() => visible.value.slice(0, shownCount.value))
// a question that was ticked and then entered the exam by another route (an import) is no longer a candidate
const chosen = computed(() => [...selected.value].filter((i) => !taken.value[i]))
const cap = computed(() => capState(props.existing.length, chosen.value.length, MAX_EXAM_QUESTIONS))

watch(bankId, () => { selected.value = new Set(); search.value = ''; shownCount.value = SHOW_STEP; capped.value = false })
watch(search, () => { shownCount.value = SHOW_STEP })
// a bank removed elsewhere (or the first load after the store filled): keep the choice valid
watch(banks, (list) => { if (!list.some((b) => b.id === bankId.value)) bankId.value = list[0]?.id ?? '' })

function toggle(i: number) {
  const next = new Set(selected.value)
  if (!next.delete(i)) next.add(i)
  selected.value = next
  capped.value = false
}
function selectAll() {
  const r = selectAllWithin(new Set(chosen.value), visible.value, taken.value, MAX_EXAM_QUESTIONS - props.existing.length - chosen.value.length)
  selected.value = r.next
  capped.value = r.skipped > 0
}
function selectNone() {
  selected.value = selectNoneOf(selected.value, visible.value)
  capped.value = false
}
function add() {
  const qs = chosenQuestions(questions.value, new Set(chosen.value))
  if (!qs.length || cap.value.over > 0) return
  emit('add', qs)
  notice.value = `${pt('lbAdded')} ${qs.length}`
  selected.value = new Set()
  capped.value = false
}

const isCorrect = (q: Question, i: number) => (q.type === 'single_choice' || q.type === 'multi_choice' ? (q.answer ?? '').includes(letter(i)) : q.type === 'true_false' ? q.answer === letter(i) : false)
const options = (q: Question) => (q.type === 'true_false' && !(q.options ?? []).length ? ['صحيح', 'خطأ'] : q.options ?? [])
const isDisabled = (i: number) => taken.value[i] || (!selected.value.has(i) && cap.value.full)
</script>

<template>
  <section class="card-filled p-5 space-y-3" data-testid="local-bank-picker">
    <div>
      <h2 class="text-title-md font-bold">{{ pt('lbTitle') }}</h2>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('lbHint') }}</p>
    </div>
    <!-- the banks are not part of the account: they live in this browser only -->
    <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="bank-local-note">{{ pt('edBankLocalNote') }}</p>

    <p v-if="!banks.length" class="text-body-md" data-testid="lbank-empty">
      {{ pt('lbNoBanks') }} <router-link to="/generate" class="underline font-semibold" data-testid="lbank-go-generate">{{ pt('lbGoGenerate') }}</router-link>
    </p>

    <template v-else>
      <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
        <label class="block">
          <span class="text-label-lg">{{ pt('lbBank') }}</span>
          <select v-model="bankId" class="input-outlined w-full mt-1" data-testid="lbank-bank">
            <option v-for="b in banks" :key="b.id" :value="b.id">{{ b.name }} ({{ b.questions.length }})</option>
          </select>
        </label>
        <label class="block">
          <span class="text-label-lg">{{ pt('lbSearch') }}</span>
          <input v-model="search" type="search" dir="auto" class="input-outlined w-full mt-1" data-testid="lbank-search" />
        </label>
      </div>

      <div class="flex flex-wrap items-center gap-2">
        <button type="button" class="btn-outlined" :disabled="!visible.length" data-testid="lbank-all" @click="selectAll">{{ pt('lbAll') }}</button>
        <button type="button" class="btn-outlined" :disabled="!visible.length" data-testid="lbank-none" @click="selectNone">{{ pt('lbNone') }}</button>
        <span class="text-body-md" data-testid="lbank-count"><b dir="ltr" class="inline-block">{{ chosen.length }}</b> {{ pt('lbSelected') }}</span>
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ visible.length }} / {{ questions.length }}</span> {{ pt('lbQuestionsIn') }}</span>
      </div>

      <p class="text-body-md" data-testid="lbank-cap">{{ pt('lbCapLabel') }}: <b dir="ltr" class="inline-block">{{ cap.total }} / {{ MAX_EXAM_QUESTIONS }}</b></p>
      <p v-if="cap.full || cap.over > 0" role="status" class="rounded-xl px-3 py-2 text-body-sm font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" data-testid="lbank-full">{{ pt('lbFull') }}</p>
      <p v-else-if="cap.near" role="status" class="rounded-xl px-3 py-2 text-body-sm font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" data-testid="lbank-near">{{ pt('lbNearCap') }}</p>
      <p v-if="capped" role="status" class="text-body-sm font-semibold" data-testid="lbank-capped">{{ pt('lbCapped') }}</p>

      <p v-if="!visible.length" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="lbank-nomatch">{{ pt('lbNoMatch') }}</p>
      <ul class="space-y-2">
        <li v-for="i in shown" :key="i" class="card-elevated p-3 flex gap-3 items-start" data-testid="lbank-item">
          <input
            type="checkbox" class="mt-1 h-5 w-5 shrink-0" :checked="selected.has(i) && !taken[i]" :disabled="isDisabled(i)"
            :aria-label="`${i + 1}. ${(questions[i]?.stem ?? '').slice(0, 40)}`" :data-testid="`lbank-q-${i}`" @change="toggle(i)"
          />
          <div class="flex-1 min-w-0 space-y-1">
            <div class="flex flex-wrap gap-2 text-xs font-semibold">
              <span class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(`bank_t_${questions[i]!.type}` as PlatformKey) }}</span>
              <span v-if="questions[i]!.chapter" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ questions[i]!.chapter }}</span>
              <span v-if="taken[i]" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" data-testid="lbank-taken">{{ pt('lbTaken') }}</span>
            </div>
            <div class="whitespace-pre-wrap break-words font-semibold" dir="auto">{{ questions[i]!.stem }}</div>
            <ol v-if="options(questions[i]!).length" class="text-body-sm space-y-0.5">
              <li v-for="(o, k) in options(questions[i]!)" :key="k" class="flex gap-2" :class="isCorrect(questions[i]!, k) ? 'font-bold' : ''">
                <span dir="ltr" class="inline-block w-6 shrink-0">{{ letter(k) }}.</span><span class="break-words" dir="auto">{{ o }}</span><span v-if="isCorrect(questions[i]!, k)" aria-hidden="true">✓</span>
              </li>
            </ol>
          </div>
        </li>
      </ul>
      <button v-if="visible.length > shownCount" type="button" class="btn-outlined" data-testid="lbank-more" @click="shownCount += SHOW_STEP">{{ pt('lbMore') }}</button>

      <p v-if="notice" role="status" class="text-body-md font-semibold" data-testid="lbank-notice">{{ notice }}</p>
      <button type="button" class="btn-filled" :disabled="!chosen.length || cap.over > 0" data-testid="lbank-add" @click="add">
        {{ pt('lbAdd') }} (<span dir="ltr" class="inline-block">{{ chosen.length }}</span>)
      </button>
    </template>
  </section>
</template>
