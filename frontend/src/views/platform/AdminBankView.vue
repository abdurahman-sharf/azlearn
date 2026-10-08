<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import SubjectPicker from '@/components/platform/SubjectPicker.vue'
import BankItemForm from '@/components/platform/BankItemForm.vue'
import BankImportPanel from '@/components/platform/BankImportPanel.vue'
import {
  BANK_DIFFICULTIES, BANK_TYPES, bankFacets, bulkBank, createBankItem, listBank, updateBankItem,
  type BankItem, type BankItemInput, type Facets,
} from '@/api/platformBank'

const pt = usePt()
const PAGE = 25

const subjectId = ref('')
const items = ref<BankItem[]>([])
const total = ref(0)
const facets = ref<Facets | null>(null)
const page = ref(0)
const loading = ref(false)
const error = ref('')
const notice = ref('')
const selected = ref(new Set<string>())
const editing = ref<BankItem | 'new' | null>(null)
const showImport = ref(false)

const filter = reactive({ type: '', difficulty: '', chapter: '', tag: '', q: '', state: 'active' as 'active' | 'archived' | 'all' })
const search = ref('')
const pages = computed(() => Math.max(1, Math.ceil(total.value / PAGE)))

async function load() {
  if (!subjectId.value) { items.value = []; total.value = 0; facets.value = null; return }
  loading.value = true
  error.value = ''
  try {
    const [list, f] = await Promise.all([
      listBank({ subject_id: subjectId.value, ...filter, limit: PAGE, offset: page.value * PAGE }),
      bankFacets(subjectId.value),
    ])
    items.value = list.items
    total.value = list.total
    facets.value = f
    // A filter that no longer has any value (e.g. the last tagged question was archived) is dropped.
    selected.value = new Set([...selected.value].filter((id) => list.items.some((i) => i.id === id)))
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}
const refilter = () => { page.value = 0; selected.value = new Set(); load() }
function submitSearch() { filter.q = search.value; refilter() }
watch(subjectId, () => {
  Object.assign(filter, { type: '', difficulty: '', chapter: '', tag: '', q: '', state: 'active' })
  search.value = ''
  editing.value = null
  showImport.value = false
  notice.value = ''
  refilter()
})
const go = (d: number) => { page.value += d; load() }

async function save(input: BankItemInput) {
  error.value = ''
  notice.value = ''
  try {
    const r = editing.value === 'new' ? await createBankItem(subjectId.value, input) : await updateBankItem((editing.value as BankItem).id, input)
    notice.value = r.duplicate_of ? pt('bankSavedDuplicate') : pt('bankSaved')
    editing.value = null
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function act(ids: string[], action: 'archive' | 'restore' | 'delete') {
  if (!ids.length) return
  if (action === 'delete' && !window.confirm(pt('bankDeleteConfirm'))) return
  error.value = ''
  notice.value = ''
  try {
    await bulkBank(ids, action)
    notice.value = pt('bankDone')
    selected.value = new Set()
    // the page may have emptied out (e.g. the last item archived): step back
    if (page.value > 0 && ids.length >= items.value.length) page.value -= 1
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

function toggleSel(id: string) {
  const s = new Set(selected.value)
  if (!s.delete(id)) s.add(id)
  selected.value = s
}
const allOnPage = computed(() => items.value.length > 0 && items.value.every((i) => selected.value.has(i.id)))
const toggleAll = () => { selected.value = allOnPage.value ? new Set() : new Set(items.value.map((i) => i.id)) }

const letter = (i: number) => String.fromCharCode(65 + i)
const isCorrect = (it: BankItem, i: number) => (it.type === 'single_choice' || it.type === 'multi_choice' ? it.answer.includes(letter(i)) : it.type === 'true_false' ? it.answer === letter(i) : false)
const tfOptions = (it: BankItem) => (it.options.length ? it.options : [pt('bankFormTrue'), pt('bankFormFalse')])
const shownOptions = (it: BankItem) => (it.type === 'true_false' ? tfOptions(it) : it.options)
const typeLabel = (t: string) => pt(`bank_t_${t}` as PlatformKey)
const diffLabel = (d: string) => (d === 'none' ? pt('bankNoDifficulty') : pt(`bank_d_${d}` as PlatformKey))
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="bank-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight">{{ pt('bankTitle') }}</h1>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('bankDesc') }}</p>
    </div>

    <SubjectPicker v-model="subjectId" />
    <p v-if="!subjectId" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="bank-choose">{{ pt('bankChooseSubject') }}</p>

    <template v-else>
      <div class="flex flex-wrap items-center gap-2">
        <button class="btn-filled" data-testid="bank-add" @click="editing = 'new'; showImport = false">{{ pt('bankAdd') }}</button>
        <button class="btn-tonal" data-testid="bank-import" @click="showImport = !showImport; editing = null">{{ pt('bankImport') }}</button>
        <span v-if="facets" class="text-body-sm ms-auto" style="color: rgb(var(--md-on-surface-variant))" data-testid="bank-counts">{{ pt('bankCount') }}: <b dir="ltr" class="inline-block">{{ facets.active }}</b></span>
      </div>

      <BankImportPanel v-if="showImport" :subject-id="subjectId" @close="showImport = false" @done="load" />
      <BankItemForm v-if="editing" :key="editing === 'new' ? 'new' : editing.id" :item="editing === 'new' ? null : editing" @save="save" @cancel="editing = null" />

      <p v-if="notice" role="status" class="text-body-md" data-testid="bank-notice">{{ notice }}</p>
      <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="bank-error">{{ error }}</p>

      <!-- filters -->
      <form class="grid grid-cols-2 md:grid-cols-4 gap-2" @submit.prevent="submitSearch">
        <input v-model="search" type="search" :placeholder="pt('bankSearch')" class="input-outlined col-span-2 md:col-span-4" data-testid="bank-search" />
        <select v-model="filter.type" class="input-outlined" :aria-label="pt('bankFormType')" data-testid="f-type" @change="refilter">
          <option value="">{{ pt('bankAllTypes') }}</option>
          <option v-for="t in BANK_TYPES" :key="t" :value="t">{{ typeLabel(t) }}</option>
        </select>
        <select v-model="filter.difficulty" class="input-outlined" :aria-label="pt('bankFormDifficulty')" data-testid="f-difficulty" @change="refilter">
          <option value="">{{ pt('bankAllDifficulties') }}</option>
          <option v-for="d in BANK_DIFFICULTIES" :key="d" :value="d">{{ diffLabel(d) }}</option>
          <option value="none">{{ pt('bankNoDifficulty') }}</option>
        </select>
        <select v-model="filter.chapter" class="input-outlined" :aria-label="pt('bankFormChapter')" data-testid="f-chapter" @change="refilter">
          <option value="">{{ pt('bankAllChapters') }}</option>
          <option v-for="c in facets?.chapters ?? []" :key="c.value" :value="c.value">{{ c.value }} ({{ c.count }})</option>
        </select>
        <select v-model="filter.tag" class="input-outlined" :aria-label="pt('bankFormTags')" data-testid="f-tag" @change="refilter">
          <option value="">{{ pt('bankAllTags') }}</option>
          <option v-for="t in facets?.tags ?? []" :key="t.value" :value="t.value">{{ t.value }} ({{ t.count }})</option>
        </select>
      </form>
      <div class="flex flex-wrap gap-2 items-center">
        <button v-for="s in (['active', 'archived', 'all'] as const)" :key="s" :class="filter.state === s ? 'btn-filled' : 'btn-outlined'" :data-testid="'state-' + s" @click="filter.state = s; refilter()">
          {{ pt(s === 'active' ? 'bankStateActive' : s === 'archived' ? 'bankStateArchived' : 'bankStateAll') }}<template v-if="facets && s !== 'all'"> (<span dir="ltr">{{ s === 'active' ? facets.active : facets.archived }}</span>)</template>
        </button>
        <label class="flex items-center gap-2 ms-auto text-body-sm"><input type="checkbox" :checked="allOnPage" data-testid="bank-select-all" @change="toggleAll" /> {{ pt('bankSelectPage') }}</label>
      </div>
      <div v-if="selected.size" class="card-elevated p-3 flex flex-wrap items-center gap-2" data-testid="bulk-bar">
        <span class="text-body-md"><b dir="ltr" class="inline-block" data-testid="sel-count">{{ selected.size }}</b> {{ pt('bankSelected') }}</span>
        <button v-if="filter.state !== 'archived'" class="btn-tonal" data-testid="bulk-archive" @click="act([...selected], 'archive')">{{ pt('bankArchive') }}</button>
        <button v-if="filter.state !== 'active'" class="btn-tonal" data-testid="bulk-restore" @click="act([...selected], 'restore')">{{ pt('bankRestore') }}</button>
        <button class="btn-outlined" data-testid="bulk-delete" @click="act([...selected], 'delete')">{{ pt('bankDelete') }}</button>
      </div>

      <p v-if="!loading && !items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="bank-empty">{{ pt('bankEmpty') }}</p>

      <ul class="space-y-3">
        <li v-for="it in items" :key="it.id" class="card-filled p-4 space-y-2" :class="it.archived_at ? 'opacity-70' : ''" data-testid="bank-item">
          <div class="flex items-start gap-3">
            <input type="checkbox" class="mt-1 h-5 w-5 shrink-0" :checked="selected.has(it.id)" :aria-label="it.stem.slice(0, 40)" data-testid="item-select" @change="toggleSel(it.id)" />
            <div class="min-w-0 flex-1 space-y-2">
              <div class="flex flex-wrap gap-2 text-xs font-semibold">
                <span class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ typeLabel(it.type) }}</span>
                <span v-if="it.difficulty" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ diffLabel(it.difficulty) }}</span>
                <span v-if="it.chapter" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ it.chapter }}</span>
                <span v-for="t in it.tags" :key="t" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))">#{{ t }}</span>
                <span v-if="it.archived_at" class="px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt('bankStateArchived') }}</span>
              </div>
              <div class="whitespace-pre-wrap break-words font-semibold" dir="auto" data-testid="item-stem">{{ it.stem }}</div>
              <ol v-if="shownOptions(it).length" class="space-y-1">
                <li v-for="(o, i) in shownOptions(it)" :key="i" class="flex gap-2" :class="isCorrect(it, i) ? 'font-bold' : ''">
                  <span dir="ltr" class="inline-block w-6 shrink-0">{{ letter(i) }}.</span>
                  <span class="flex-1 break-words" dir="auto">{{ o }}</span>
                  <span v-if="isCorrect(it, i)" aria-hidden="true">✓</span>
                </li>
              </ol>
              <div v-else class="text-body-md" dir="auto"><span class="font-bold">✓</span> {{ it.answer }}</div>
              <details v-if="it.analysis"><summary class="cursor-pointer text-body-sm">{{ pt('bankFormAnalysis') }}</summary><p class="text-body-sm whitespace-pre-wrap mt-1" dir="auto">{{ it.analysis }}</p></details>
              <div class="flex gap-2">
                <button class="btn-text" data-testid="item-edit" @click="editing = it; showImport = false">{{ pt('bankEdit') }}</button>
                <button v-if="!it.archived_at" class="btn-text" data-testid="item-archive" @click="act([it.id], 'archive')">{{ pt('bankArchive') }}</button>
                <button v-else class="btn-text" data-testid="item-restore" @click="act([it.id], 'restore')">{{ pt('bankRestore') }}</button>
                <button class="btn-text" data-testid="item-delete" @click="act([it.id], 'delete')">{{ pt('bankDelete') }}</button>
              </div>
            </div>
          </div>
        </li>
      </ul>

      <nav v-if="pages > 1" class="flex items-center justify-between gap-3" data-testid="bank-pager">
        <button class="btn-outlined" :disabled="page === 0 || loading" data-testid="bank-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
        <span class="text-body-sm">{{ pt('bankPage') }} <span dir="ltr" class="inline-block">{{ page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('bankFound') }}</span>
        <button class="btn-outlined" :disabled="page + 1 >= pages || loading" data-testid="bank-next" @click="go(1)">{{ pt('nextPage') }}</button>
      </nav>
    </template>
  </div>
</template>
