<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import {
  getStructure, createUnit, updateUnit, deleteUnit, createSubject, deleteSubject,
  type Structure, type Unit, type UnitKind,
} from '@/api/platformAdmin'

const pt = usePt()
const route = useRoute()
const id = route.params.id as string

const data = ref<Structure | null>(null)
const error = ref('')

const KINDS: { kind: UnitKind; rank: number; key: PlatformKey }[] = [
  { kind: 'department', rank: 0, key: 'kindDepartment' },
  { kind: 'level', rank: 1, key: 'kindLevel' },
  { kind: 'year', rank: 2, key: 'kindYear' },
  { kind: 'term', rank: 3, key: 'kindTerm' },
]
const rankOf = (k: UnitKind) => KINDS.find(x => x.kind === k)!.rank
const kindLabel = (k: UnitKind) => pt(KINDS.find(x => x.kind === k)!.key)

/** Kinds allowed under `parent` (null = root); schools have no departments. */
function allowedKinds(parent: Unit | null) {
  const isSchool = data.value?.institution.type === 'school'
  return KINDS.filter(k => (!isSchool || k.kind !== 'department') && (!parent || k.rank > rankOf(parent.kind)))
}

const rows = computed(() => {
  const units = data.value?.units ?? []
  const out: { unit: Unit; depth: number }[] = []
  const walk = (parent: string | null, depth: number) => {
    for (const u of units.filter(x => x.parent_id === parent)) {
      out.push({ unit: u, depth })
      walk(u.id, depth + 1)
    }
  }
  walk(null, 0)
  return out
})
const subjectsOf = (unitId: string | null) => (data.value?.subjects ?? []).filter(s => s.unit_id === unitId)

// A single inline form at a time.
const form = ref<{ type: 'unit' | 'subject'; parent: Unit | null; kind: UnitKind; name: string } | null>(null)

function openUnitForm(parent: Unit | null) {
  form.value = { type: 'unit', parent, kind: allowedKinds(parent)[0]?.kind ?? 'level', name: '' }
}
function openSubjectForm(parent: Unit | null) {
  form.value = { type: 'subject', parent, kind: 'level', name: '' }
}
const isFormAt = (t: 'unit' | 'subject', parent: Unit | null) =>
  form.value?.type === t && (form.value.parent?.id ?? null) === (parent?.id ?? null)

async function run(fn: () => Promise<unknown>) {
  error.value = ''
  try {
    await fn()
    data.value = await getStructure(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function submit() {
  const f = form.value
  if (!f) return
  await run(async () => {
    if (f.type === 'unit') await createUnit({ institution_id: id, parent_id: f.parent?.id, kind: f.kind, name_ar: f.name })
    else await createSubject({ institution_id: id, unit_id: f.parent?.id, name_ar: f.name })
    form.value = null
  })
}

const toggleUnit = (u: Unit) => run(() => updateUnit(u.id, { is_active: !u.is_active }))
const removeUnit = (u: Unit) => window.confirm(pt('confirmDelete')) && run(() => deleteUnit(u.id))
const removeSubject = (sid: string) => window.confirm(pt('confirmDelete')) && run(() => deleteSubject(sid))

onMounted(() => run(async () => {}))
</script>

<template>
  <div v-if="data" class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform/admin/institutions" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3">{{ data.institution.name_ar }}</h1>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ pt('structure') }}</p>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <!-- inline add form (shared) -->
    <form v-if="isFormAt('unit', null) || isFormAt('subject', null)" class="card-elevated p-3 mb-3 flex flex-wrap gap-2" @submit.prevent="submit">
        <select v-if="form!.type === 'unit'" v-model="form!.kind" class="input-outlined !py-2">
          <option v-for="k in allowedKinds(null)" :key="k.kind" :value="k.kind">{{ pt(k.key) }}</option>
        </select>
        <input v-model="form!.name" required maxlength="200" :placeholder="pt('nameAr')" class="input-outlined flex-1 min-w-[10rem]" />
        <button type="submit" class="btn-filled">{{ pt('save') }}</button>
        <button type="button" class="btn-text" @click="form = null">{{ pt('cancel') }}</button>
    </form>

    <div class="flex gap-2 mb-4">
      <button class="btn-tonal" @click="openUnitForm(null)">{{ pt('addUnit') }}</button>
      <button class="btn-outlined" @click="openSubjectForm(null)">{{ pt('addSubject') }}</button>
    </div>

    <ul class="space-y-2">
      <li v-for="{ unit, depth } in rows" :key="unit.id">
        <div class="card-filled p-3" :class="{ 'opacity-60': !unit.is_active }" :style="{ marginInlineStart: `${depth * 1.25}rem` }">
          <div class="flex items-center gap-2">
            <span class="shrink-0 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-secondary-container))">{{ kindLabel(unit.kind) }}</span>
            <span class="font-bold break-words min-w-0">{{ unit.name_ar }}</span>
          </div>
          <div class="flex flex-wrap items-center gap-x-1 mt-1">
            <button v-if="allowedKinds(unit).length" class="btn-text" @click="openUnitForm(unit)">{{ pt('addUnit') }}</button>
            <button class="btn-text" @click="openSubjectForm(unit)">{{ pt('addSubject') }}</button>
            <button class="btn-text" @click="toggleUnit(unit)">{{ pt('toggleActive') }}</button>
            <button class="btn-text" @click="removeUnit(unit)">{{ pt('del') }}</button>
          </div>
          <div v-if="subjectsOf(unit.id).length" class="flex flex-wrap gap-2 mt-2">
            <span v-for="s in subjectsOf(unit.id)" :key="s.id" class="inline-flex items-center gap-1 px-3 py-1 rounded-full text-sm" style="background-color: rgb(var(--md-surface-container-high))">
              {{ s.name_ar }}
              <button class="font-bold" :aria-label="pt('del')" @click="removeSubject(s.id)">×</button>
            </span>
          </div>
          <form v-if="isFormAt('unit', unit) || isFormAt('subject', unit)" class="mt-3 flex flex-wrap gap-2" @submit.prevent="submit">
            <select v-if="form!.type === 'unit'" v-model="form!.kind" class="input-outlined !py-2">
              <option v-for="k in allowedKinds(unit)" :key="k.kind" :value="k.kind">{{ pt(k.key) }}</option>
            </select>
            <input v-model="form!.name" required maxlength="200" :placeholder="pt('nameAr')" class="input-outlined flex-1 min-w-[10rem]" />
            <button type="submit" class="btn-filled">{{ pt('save') }}</button>
            <button type="button" class="btn-text" @click="form = null">{{ pt('cancel') }}</button>
          </form>
        </div>
      </li>
    </ul>

    <div v-if="subjectsOf(null).length" class="mt-5">
      <div class="font-semibold mb-2">{{ pt('subjects') }}</div>
      <div class="flex flex-wrap gap-2">
        <span v-for="s in subjectsOf(null)" :key="s.id" class="inline-flex items-center gap-1 px-3 py-1 rounded-full text-sm" style="background-color: rgb(var(--md-surface-container-high))">
          {{ s.name_ar }}
          <button class="font-bold" :aria-label="pt('del')" @click="removeSubject(s.id)">×</button>
        </span>
      </div>
    </div>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
