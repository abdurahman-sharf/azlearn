<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { BuildingLibraryIcon, PlusIcon } from '@heroicons/vue/24/outline'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import type { InstitutionType } from '@/stores/auth'
import { listInstitutions, createInstitution, updateInstitution, deleteInstitution, type Institution } from '@/api/platformAdmin'
import { institutionStats, type InstitutionStat } from '@/api/platformStats'
import StructureCard from '@/components/platform/StructureCard.vue'
import StructureDialog, { type StructureValues } from '@/components/platform/StructureDialog.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

const pt = usePt()
const type = ref<InstitutionType>('school')
const items = ref<Institution[]>([])
const stats = ref<Map<string, InstitutionStat>>(new Map())
const loading = ref(true)
const error = ref('')

const types: { value: InstitutionType; key: PlatformKey }[] = [
  { value: 'school', key: 'typeSchool' },
  { value: 'institute', key: 'typeInstitute' },
  { value: 'university', key: 'typeUniversity' },
]

async function load() {
  error.value = ''
  try {
    const [list, st] = await Promise.all([listInstitutions(type.value), institutionStats().catch(() => [] as InstitutionStat[])])
    items.value = list
    stats.value = new Map(st.map((s) => [s.institution_id, s]))
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}
const numbers = (i: Institution) => stats.value.get(i.id) ?? { institution_id: i.id, subjects: 0, exams: 0, attempts: 0, questions: 0 }
const cardStats = (i: Institution) => [
  { label: pt('subjects'), value: numbers(i).subjects },
  { label: pt('stcAttempts'), value: numbers(i).attempts },
  { label: pt('stcQuestions'), value: numbers(i).questions },
]

function pick(t: InstitutionType) {
  type.value = t
  load()
}

// ── create / edit / hide / delete
const dialog = ref<{ mode: 'create' } | { mode: 'edit'; item: Institution } | null>(null)
const removing = ref<Institution | null>(null)
const busy = ref(false)
const dialogError = ref('')

async function run(fn: () => Promise<unknown>): Promise<boolean> {
  busy.value = true
  dialogError.value = ''
  try {
    await fn()
    await load()
    return true
  } catch (e) {
    dialogError.value = platformErrorMessage(pt, e)
    return false
  } finally {
    busy.value = false
  }
}

async function save(v: StructureValues) {
  const d = dialog.value
  if (!d) return
  const ok = await run(() =>
    d.mode === 'create'
      ? createInstitution({ type: type.value, name_ar: v.name_ar, name_en: v.name_en || undefined, city: v.city || undefined })
      : updateInstitution(d.item.id, { name_ar: v.name_ar, name_en: v.name_en, city: v.city }),
  )
  if (ok) dialog.value = null
}

async function toggle(i: Institution) {
  error.value = ''
  try {
    await updateInstitution(i.id, { is_active: !i.is_active })
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function confirmRemove() {
  const i = removing.value
  if (!i) return
  if (await run(() => deleteInstitution(i.id))) removing.value = null
}

const removeCounts = computed(() => {
  const n = removing.value ? numbers(removing.value) : null
  return n ? [
    { label: pt('subjects'), value: n.subjects },
    { label: pt('stcExams'), value: n.exams },
    { label: pt('stcAttempts'), value: n.attempts },
    { label: pt('stcQuestions'), value: n.questions },
  ] : []
})
const dialogInitial = computed(() => {
  const d = dialog.value
  return d && d.mode === 'edit' ? { name_ar: d.item.name_ar, name_en: d.item.name_en ?? '', city: d.item.city ?? '' } : undefined
})

onMounted(load)
</script>

<template>
  <div class="max-w-5xl mx-auto pb-8" data-testid="admin-institutions">
    <div class="flex flex-wrap items-center justify-between gap-3 my-3">
      <div>
        <h1 class="text-display-sm font-bold tracking-tight">{{ pt('adminInstitutions') }}</h1>
        <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stcInstitutionsHint') }}</p>
      </div>
      <button type="button" class="btn-filled" data-testid="add-institution" @click="dialogError = ''; dialog = { mode: 'create' }">
        <PlusIcon class="w-5 h-5" aria-hidden="true" /> {{ pt('newInstitution') }}
      </button>
    </div>

    <div class="flex flex-wrap gap-2 mb-5" role="group" :aria-label="pt('stcTypeFilter')">
      <button v-for="t in types" :key="t.value" type="button" :class="type === t.value ? 'btn-filled' : 'btn-outlined'" class="flex-1 sm:flex-none sm:min-w-[8rem]" :aria-pressed="type === t.value" :data-testid="'type-' + t.value" @click="pick(t.value)">{{ pt(t.key) }}</button>
    </div>

    <p v-if="error" class="text-body-md mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="loading" role="status" class="text-body-md">{{ pt('loading') }}</p>

    <div v-else-if="!items.length" class="card-filled p-8 text-center space-y-3" data-testid="no-institutions">
      <BuildingLibraryIcon class="w-10 h-10 mx-auto" style="color: rgb(var(--md-on-surface-variant))" aria-hidden="true" />
      <p class="text-body-lg">{{ pt('noInstitutions') }}</p>
      <button type="button" class="btn-tonal" @click="dialogError = ''; dialog = { mode: 'create' }">{{ pt('newInstitution') }}</button>
    </div>

    <ul v-else class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3" data-testid="institution-cards">
      <li v-for="i in items" :key="i.id">
        <StructureCard
          class="h-full"
          :level="2"
          :title="i.name_ar"
          :subtitle="[i.name_en, i.city].filter(Boolean).join(' · ')"
          :to="`/platform/admin/institutions/${i.id}`"
          :icon="BuildingLibraryIcon"
          :inactive="!i.is_active"
          :stats="cardStats(i)"
          testid="institution-card"
          @edit="dialogError = ''; dialog = { mode: 'edit', item: i }"
          @toggle="toggle(i)"
          @remove="dialogError = ''; removing = i"
        />
      </li>
    </ul>

    <StructureDialog
      v-if="dialog"
      :title="dialog.mode === 'create' ? pt('newInstitution') : pt('stcEditInstitution')"
      :initial="dialogInitial"
      with-city
      :busy="busy"
      :error="dialogError"
      @save="save"
      @close="dialog = null"
    />
    <ConfirmDeleteDialog
      v-if="removing"
      :title="`${pt('del')}: ${removing.name_ar}`"
      :message="pt('stcDeleteInstitution')"
      :counts="removeCounts"
      :confirm-name="numbers(removing).attempts > 0 ? removing.name_ar : undefined"
      :busy="busy"
      :error="dialogError"
      @confirm="confirmRemove"
      @close="removing = null"
    />
  </div>
</template>
