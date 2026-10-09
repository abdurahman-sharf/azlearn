<script setup lang="ts">
import { reactive, ref, watch } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { BANK_DIFFICULTIES, BANK_TYPES, bankFacets, listBank, type BankItem, type Facets } from '@/api/platformBank'
import { letter } from '@/utils/examBuilder'

const props = defineProps<{ subjectId: string; taken: Set<string> }>()
const emit = defineEmits<{ add: [BankItem[]] }>()
const pt = usePt()
const PAGE = 10

const items = ref<BankItem[]>([])
const total = ref(0)
const page = ref(0)
const facets = ref<Facets | null>(null)
const selected = ref(new Map<string, BankItem>())
const error = ref('')
const search = ref('')
const filter = reactive({ type: '', difficulty: '', chapter: '', tag: '', q: '' })

async function load() {
  if (!props.subjectId) return
  error.value = ''
  try {
    const r = await listBank({ subject_id: props.subjectId, ...filter, state: 'active', limit: PAGE, offset: page.value * PAGE })
    items.value = r.items
    total.value = r.total
    if (!facets.value) facets.value = await bankFacets(props.subjectId)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
const refilter = () => { page.value = 0; load() }
function submitSearch() { filter.q = search.value; refilter() }
watch(() => props.subjectId, () => { facets.value = null; selected.value = new Map(); Object.assign(filter, { type: '', difficulty: '', chapter: '', tag: '', q: '' }); search.value = ''; refilter() }, { immediate: true })

const toggle = (it: BankItem) => { const m = new Map(selected.value); if (!m.delete(it.id)) m.set(it.id, it); selected.value = m }
function addSelected() {
  emit('add', [...selected.value.values()])
  selected.value = new Map()
}
const pages = () => Math.max(1, Math.ceil(total.value / PAGE))
const isCorrect = (it: BankItem, i: number) => (it.type === 'single_choice' || it.type === 'multi_choice' ? it.answer.includes(letter(i)) : it.type === 'true_false' ? it.answer === letter(i) : false)
</script>

<template>
  <section class="card-filled p-5 space-y-3" data-testid="bank-picker">
    <h2 class="text-title-md font-bold">{{ pt('exSourceBank') }}</h2>
    <form class="grid grid-cols-2 md:grid-cols-4 gap-2" @submit.prevent="submitSearch">
      <input v-model="search" type="search" :placeholder="pt('bankSearch')" class="input-outlined col-span-2 md:col-span-4" data-testid="pick-search" />
      <select v-model="filter.type" class="input-outlined" :aria-label="pt('bankFormType')" data-testid="pick-type" @change="refilter">
        <option value="">{{ pt('bankAllTypes') }}</option><option v-for="t in BANK_TYPES" :key="t" :value="t">{{ pt(`bank_t_${t}` as PlatformKey) }}</option>
      </select>
      <select v-model="filter.difficulty" class="input-outlined" :aria-label="pt('bankFormDifficulty')" data-testid="pick-difficulty" @change="refilter">
        <option value="">{{ pt('bankAllDifficulties') }}</option><option v-for="d in BANK_DIFFICULTIES" :key="d" :value="d">{{ pt(`bank_d_${d}` as PlatformKey) }}</option>
      </select>
      <select v-model="filter.chapter" class="input-outlined" :aria-label="pt('bankFormChapter')" data-testid="pick-chapter" @change="refilter">
        <option value="">{{ pt('bankAllChapters') }}</option><option v-for="c in facets?.chapters ?? []" :key="c.value" :value="c.value">{{ c.value }} ({{ c.count }})</option>
      </select>
      <select v-model="filter.tag" class="input-outlined" :aria-label="pt('bankFormTags')" data-testid="pick-tag" @change="refilter">
        <option value="">{{ pt('bankAllTags') }}</option><option v-for="t in facets?.tags ?? []" :key="t.value" :value="t.value">{{ t.value }} ({{ t.count }})</option>
      </select>
    </form>
    <p v-if="error" role="alert" class="text-body-sm" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!items.length && !error" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="pick-empty">{{ pt('exBankEmpty') }}</p>
    <ul class="space-y-2">
      <li v-for="it in items" :key="it.id" class="card-elevated p-3 flex gap-3 items-start" data-testid="pick-item">
        <input type="checkbox" class="mt-1 h-5 w-5 shrink-0" :checked="selected.has(it.id)" :disabled="taken.has(it.id)" :aria-label="it.stem.slice(0, 40)" data-testid="pick-select" @change="toggle(it)" />
        <div class="flex-1 min-w-0 space-y-1">
          <div class="flex flex-wrap gap-2 text-xs font-semibold">
            <span class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(`bank_t_${it.type}` as PlatformKey) }}</span>
            <span v-if="it.difficulty" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(`bank_d_${it.difficulty}` as PlatformKey) }}</span>
            <span v-if="it.chapter" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ it.chapter }}</span>
            <span v-if="taken.has(it.id)" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" data-testid="pick-taken">{{ pt('exBankIn') }}</span>
          </div>
          <div class="whitespace-pre-wrap break-words font-semibold" dir="auto">{{ it.stem }}</div>
          <ol v-if="it.options.length" class="text-body-sm space-y-0.5">
            <li v-for="(o, i) in it.options" :key="i" class="flex gap-2" :class="isCorrect(it, i) ? 'font-bold' : ''"><span dir="ltr" class="inline-block w-6 shrink-0">{{ letter(i) }}.</span><span class="break-words" dir="auto">{{ o }}</span><span v-if="isCorrect(it, i)" aria-hidden="true">✓</span></li>
          </ol>
        </div>
      </li>
    </ul>
    <nav v-if="pages() > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')">
      <button class="btn-outlined" :disabled="page === 0" data-testid="pick-prev" @click="page--; load()">{{ pt('prevPage') }}</button>
      <span class="text-body-sm" dir="ltr">{{ page + 1 }} / {{ pages() }}</span>
      <button class="btn-outlined" :disabled="page + 1 >= pages()" data-testid="pick-next" @click="page++; load()">{{ pt('nextPage') }}</button>
    </nav>
    <button class="btn-filled" :disabled="!selected.size" data-testid="pick-add" @click="addSelected">{{ pt('exBankAdd') }} (<span dir="ltr">{{ selected.size }}</span>)</button>
  </section>
</template>
