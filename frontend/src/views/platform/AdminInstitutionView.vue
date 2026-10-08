<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { BookOpenIcon, ChevronLeftIcon, PencilSquareIcon, PlusIcon, RectangleStackIcon } from '@heroicons/vue/24/outline'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import {
  getStructure, createUnit, updateUnit, deleteUnit, createSubject, updateSubject, deleteSubject, updateInstitution,
  type Structure, type Unit, type Subject,
} from '@/api/platformAdmin'
import { subjectStats, type SubjectStat } from '@/api/platformStats'
import { allowedKinds, ancestors, buildIndex, children, rollup, type SubjectNumbers } from '@/utils/structureTree'
import StructureCard from '@/components/platform/StructureCard.vue'
import StructureDialog, { type StructureValues } from '@/components/platform/StructureDialog.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

// One institution as drill-down cards: the institution shows its top level, a card opens the next level (?unit=<id>),
// and every card carries the numbers of everything below it. Subjects of the current node are cards too.
const pt = usePt()
const route = useRoute()
const id = route.params.id as string

const data = ref<Structure | null>(null)
const numbers = ref<Map<string, SubjectNumbers>>(new Map())
const error = ref('')
const loadError = ref('')

const KIND_KEY: Record<string, PlatformKey> = { department: 'kindDepartment', level: 'kindLevel', year: 'kindYear', term: 'kindTerm' }
const kindLabel = (k: string) => pt(KIND_KEY[k] ?? 'kindLevel')

const idx = computed(() => (data.value ? buildIndex(data.value.units, data.value.subjects) : null))
/** the node being viewed; an unknown or deleted ?unit falls back to the institution's top level */
const unitId = computed<string | null>(() => {
  const q = route.query.unit
  const v = typeof q === 'string' ? q : ''
  return v && idx.value?.units.has(v) ? v : null
})
const current = computed(() => (unitId.value && idx.value ? idx.value.units.get(unitId.value) ?? null : null))
const isSchool = computed(() => data.value?.institution.type === 'school')

const childUnits = computed(() => (idx.value ? children(idx.value, unitId.value) : []))
const ownSubjects = computed<Subject[]>(() => (idx.value ? idx.value.subjectsAt.get(unitId.value) ?? [] : []))
const here = computed(() => (idx.value ? rollup(idx.value, unitId.value, numbers.value) : null))

const crumbs = computed(() => {
  const out: { label: string; to?: { path: string; query?: Record<string, string> } }[] = []
  if (idx.value && current.value) {
    for (const u of [...ancestors(idx.value, current.value.id), current.value]) {
      out.push({ label: u.name_ar, to: u.id === current.value.id ? undefined : { path: route.path, query: { unit: u.id } } })
    }
  }
  return out
})

const unitStats = (u: Unit) => {
  const r = idx.value ? rollup(idx.value, u.id, numbers.value) : null
  return [
    { label: pt('subjects'), value: r?.subjects ?? 0 },
    { label: pt('stcAttempts'), value: r?.attempts ?? 0 },
    { label: pt('stcQuestions'), value: r?.questions ?? 0 },
  ]
}
/** what a card contains: its next-level names, else its own subjects */
function chipsOf(u: Unit): { chips: string[]; more: number } {
  const index = idx.value
  if (!index) return { chips: [], more: 0 }
  const names = children(index, u.id).map((c) => c.name_ar)
  const all = names.length ? names : (index.subjectsAt.get(u.id) ?? []).map((s) => s.name_ar)
  return { chips: all.slice(0, 3), more: Math.max(0, all.length - 3) }
}
const subjectStatsOf = (s: Subject) => {
  const n = numbers.value.get(s.id)
  return [
    { label: pt('stcExams'), value: n?.exams ?? 0 },
    { label: pt('stcAttempts'), value: n?.attempts ?? 0 },
    { label: pt('stcQuestions'), value: n?.questions ?? 0 },
  ]
}

async function load() {
  loadError.value = ''
  try {
    const [structure, st] = await Promise.all([getStructure(id), subjectStats(id).catch(() => [] as SubjectStat[])])
    data.value = structure
    numbers.value = new Map(st.map((s) => [s.subject_id, { exams: s.exams, attempts: s.attempts, questions: s.questions }]))
  } catch (e) {
    loadError.value = platformErrorMessage(pt, e)
  }
}

// ── dialogs
type Dialog =
  | { kind: 'institution' }
  | { kind: 'unit-create' }
  | { kind: 'unit-edit'; unit: Unit }
  | { kind: 'subject-create' }
  | { kind: 'subject-edit'; subject: Subject }
type Removal = { kind: 'unit'; unit: Unit } | { kind: 'subject'; subject: Subject }
const dialog = ref<Dialog | null>(null)
const removal = ref<Removal | null>(null)
const busy = ref(false)
const dialogError = ref('')

const open = (d: Dialog) => { dialogError.value = ''; dialog.value = d }
const kindsHere = computed(() => allowedKinds(current.value?.kind ?? null, isSchool.value).map((k) => ({ kind: k, label: kindLabel(k) })))
const dialogTitle = computed(() => {
  switch (dialog.value?.kind) {
    case 'institution': return pt('stcEditInstitution')
    case 'unit-create': return pt('addUnit')
    case 'unit-edit': return `${pt('edit')}: ${dialog.value.unit.name_ar}`
    case 'subject-create': return pt('addSubject')
    case 'subject-edit': return `${pt('edit')}: ${dialog.value.subject.name_ar}`
    default: return ''
  }
})
const dialogInitial = computed(() => {
  const d = dialog.value
  if (!d) return undefined
  if (d.kind === 'institution' && data.value) return { name_ar: data.value.institution.name_ar, name_en: data.value.institution.name_en ?? '', city: data.value.institution.city ?? '' }
  if (d.kind === 'unit-edit') return { name_ar: d.unit.name_ar, name_en: d.unit.name_en ?? '' }
  if (d.kind === 'subject-edit') return { name_ar: d.subject.name_ar, name_en: d.subject.name_en ?? '' }
  return undefined
})

async function run(fn: () => Promise<unknown>): Promise<boolean> {
  busy.value = true
  dialogError.value = ''
  error.value = ''
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
  const ok = await run(() => {
    switch (d.kind) {
      case 'institution': return updateInstitution(id, { name_ar: v.name_ar, name_en: v.name_en, city: v.city })
      case 'unit-create': return createUnit({ institution_id: id, parent_id: unitId.value ?? undefined, kind: v.kind as Unit['kind'], name_ar: v.name_ar, name_en: v.name_en || undefined })
      case 'unit-edit': return updateUnit(d.unit.id, { name_ar: v.name_ar, name_en: v.name_en })
      case 'subject-create': return createSubject({ institution_id: id, unit_id: unitId.value ?? undefined, name_ar: v.name_ar, name_en: v.name_en || undefined })
      case 'subject-edit': return updateSubject(d.subject.id, { name_ar: v.name_ar, name_en: v.name_en })
    }
  })
  if (ok) dialog.value = null
}

async function toggle(fn: () => Promise<unknown>) {
  const ok = await run(fn)
  if (!ok) error.value = dialogError.value
}
const toggleUnit = (u: Unit) => toggle(() => updateUnit(u.id, { is_active: !u.is_active }))
const toggleSubject = (s: Subject) => toggle(() => updateSubject(s.id, { is_active: !s.is_active }))

async function confirmRemove() {
  const r = removal.value
  if (!r) return
  if (await run(() => (r.kind === 'unit' ? deleteUnit(r.unit.id) : deleteSubject(r.subject.id)))) removal.value = null
}
const removalName = computed(() => (removal.value ? (removal.value.kind === 'unit' ? removal.value.unit.name_ar : removal.value.subject.name_ar) : ''))
const removalRoll = computed(() => {
  const r = removal.value
  if (!r || !idx.value) return null
  if (r.kind === 'unit') return rollup(idx.value, r.unit.id, numbers.value)
  const n = numbers.value.get(r.subject.id)
  return { childUnits: 0, descendantUnits: 0, subjects: 1, exams: n?.exams ?? 0, attempts: n?.attempts ?? 0, questions: n?.questions ?? 0 }
})
const removalCounts = computed(() => {
  const r = removal.value
  const n = removalRoll.value
  if (!r || !n) return []
  return r.kind === 'unit'
    ? [{ label: pt('stcSubLevels'), value: n.descendantUnits }]
    : [{ label: pt('stcExams'), value: n.exams }, { label: pt('stcAttempts'), value: n.attempts }, { label: pt('stcQuestions'), value: n.questions }]
})
// deleting a level never deletes its subjects (they move to the institution's top level) — say so
const removalKept = computed(() => (removal.value?.kind === 'unit' && removalRoll.value?.subjects ? `${pt('stcKeptSubjects')} (${removalRoll.value.subjects})` : undefined))

watch(() => route.query.unit, () => { error.value = '' })
onMounted(load)
</script>

<template>
  <div v-if="data" class="max-w-5xl mx-auto pb-8" data-testid="admin-institution">
    <nav :aria-label="pt('stcBreadcrumb')" class="mb-3">
      <ol class="flex flex-wrap items-center gap-1 text-body-md">
        <li><router-link to="/platform/admin/institutions" class="underline" data-testid="crumb-root">{{ pt('adminInstitutions') }}</router-link></li>
        <li class="flex items-center gap-1">
          <ChevronLeftIcon class="w-4 h-4 rtl:rotate-0 ltr:rotate-180" aria-hidden="true" />
          <router-link v-if="current" :to="{ path: route.path }" class="underline" data-testid="crumb-institution">{{ data.institution.name_ar }}</router-link>
          <span v-else aria-current="page" class="font-bold">{{ data.institution.name_ar }}</span>
        </li>
        <li v-for="(c, n) in crumbs" :key="n" class="flex items-center gap-1">
          <ChevronLeftIcon class="w-4 h-4 rtl:rotate-0 ltr:rotate-180" aria-hidden="true" />
          <router-link v-if="c.to" :to="c.to" class="underline">{{ c.label }}</router-link>
          <span v-else aria-current="page" class="font-bold">{{ c.label }}</span>
        </li>
      </ol>
    </nav>

    <div class="flex flex-wrap items-start justify-between gap-3 mb-4">
      <div class="min-w-0">
        <h1 class="text-display-sm font-bold tracking-tight break-words" data-testid="node-title">{{ current ? current.name_ar : data.institution.name_ar }}</h1>
        <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ current ? kindLabel(current.kind) : pt('structure') }}</p>
      </div>
      <div class="flex flex-wrap gap-2">
        <button v-if="!current" type="button" class="btn-outlined" data-testid="edit-institution" @click="open({ kind: 'institution' })">
          <PencilSquareIcon class="w-5 h-5" aria-hidden="true" /> {{ pt('stcEditInstitution') }}
        </button>
        <button v-if="kindsHere.length" type="button" class="btn-tonal" data-testid="add-unit" @click="open({ kind: 'unit-create' })">
          <PlusIcon class="w-5 h-5" aria-hidden="true" /> {{ pt('addUnit') }}
        </button>
        <button type="button" class="btn-filled" data-testid="add-subject" @click="open({ kind: 'subject-create' })">
          <PlusIcon class="w-5 h-5" aria-hidden="true" /> {{ pt('addSubject') }}
        </button>
      </div>
    </div>

    <p v-if="error" class="text-body-md mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <ul v-if="here" class="grid grid-cols-2 sm:grid-cols-4 gap-2 mb-6" data-testid="node-totals">
      <li class="card-filled p-3"><span class="block text-body-sm">{{ pt('stcSubLevels') }}</span><b class="text-title-lg" dir="ltr">{{ here.descendantUnits }}</b></li>
      <li class="card-filled p-3"><span class="block text-body-sm">{{ pt('subjects') }}</span><b class="text-title-lg" dir="ltr">{{ here.subjects }}</b></li>
      <li class="card-filled p-3"><span class="block text-body-sm">{{ pt('stcAttempts') }}</span><b class="text-title-lg" dir="ltr">{{ here.attempts }}</b></li>
      <li class="card-filled p-3"><span class="block text-body-sm">{{ pt('stcQuestions') }}</span><b class="text-title-lg" dir="ltr">{{ here.questions }}</b></li>
    </ul>

    <section v-if="childUnits.length" class="mb-6" aria-labelledby="sec-units">
      <h2 id="sec-units" class="text-title-md font-bold mb-3">{{ current ? pt('stcNextLevel') : pt('stcTopLevel') }}</h2>
      <ul class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3" data-testid="unit-cards">
        <li v-for="u in childUnits" :key="u.id">
          <StructureCard
            class="h-full"
            :title="u.name_ar"
            :subtitle="u.name_en ?? undefined"
            :badge="kindLabel(u.kind)"
            :icon="RectangleStackIcon"
            :to="{ path: route.path, query: { unit: u.id } }"
            :inactive="!u.is_active"
            :stats="unitStats(u)"
            :chips="chipsOf(u).chips"
            :more-chips="chipsOf(u).more"
            testid="unit-card"
            @edit="open({ kind: 'unit-edit', unit: u })"
            @toggle="toggleUnit(u)"
            @remove="dialogError = ''; removal = { kind: 'unit', unit: u }"
          />
        </li>
      </ul>
    </section>

    <section v-if="ownSubjects.length" class="mb-6" aria-labelledby="sec-subjects">
      <h2 id="sec-subjects" class="text-title-md font-bold mb-3">{{ pt('subjects') }}</h2>
      <ul class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3" data-testid="subject-cards">
        <li v-for="s in ownSubjects" :key="s.id">
          <StructureCard
            class="h-full"
            :title="s.name_ar"
            :subtitle="s.name_en ?? undefined"
            :icon="BookOpenIcon"
            :to="`/platform/subjects/${s.id}`"
            :inactive="!s.is_active"
            :stats="subjectStatsOf(s)"
            testid="subject-card"
            @edit="open({ kind: 'subject-edit', subject: s })"
            @toggle="toggleSubject(s)"
            @remove="dialogError = ''; removal = { kind: 'subject', subject: s }"
          />
        </li>
      </ul>
    </section>

    <div v-if="!childUnits.length && !ownSubjects.length" class="card-filled p-8 text-center space-y-3" data-testid="node-empty">
      <p class="text-body-lg">{{ pt('stcEmptyNode') }}</p>
    </div>

    <StructureDialog
      v-if="dialog"
      :title="dialogTitle"
      :initial="dialogInitial"
      :kinds="dialog.kind === 'unit-create' ? kindsHere : undefined"
      :with-city="dialog.kind === 'institution'"
      :busy="busy"
      :error="dialogError"
      @save="save"
      @close="dialog = null"
    />
    <ConfirmDeleteDialog
      v-if="removal"
      :title="`${pt('del')}: ${removalName}`"
      :message="removal.kind === 'unit' ? pt('stcDeleteUnit') : pt('stcDeleteSubject')"
      :counts="removalCounts"
      :kept="removalKept"
      :confirm-name="removal.kind === 'subject' && (removalRoll?.attempts ?? 0) > 0 ? removalName : undefined"
      :busy="busy"
      :error="dialogError"
      @confirm="confirmRemove"
      @close="removal = null"
    />
  </div>
  <p v-else-if="loadError" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ loadError }}</p>
  <p v-else class="max-w-3xl mx-auto" role="status">{{ pt('loading') }}</p>
</template>
