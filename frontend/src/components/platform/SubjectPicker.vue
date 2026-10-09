<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { getStructure, listInstitutions, type Institution, type Structure, type Unit } from '@/api/platformAdmin'
import { buildIndex, children, pathToSubject, subtreeSubjects } from '@/utils/structureTree'
import { activeSubset, restrictToSubjects, subjectSelectable, type AssignmentStatus } from '@/utils/subjectPicker'

// Institution → level/department/year/term (one list per depth, as deep as the structure goes) → subject. The model is
// the subject id; the last subject is remembered per browser (`remember`, admin screens only: with `remember=false` the
// picker neither reads nor writes that key).
// - `statusBySubject`: a teacher's assignment per subject id. Subjects that are pending or approved are listed with
//   their status and cannot be picked (they are already requested); rejected ones stay pickable for a new request.
// - `activeOnly`: hide inactive institutions, units and subjects (and everything below an inactive unit).
// - `allowedSubjectIds`: offer only these subjects (a teacher's approved ones) and the units on the way to them.
//   `subjectInstitutions` (subject id → institution id) lets the picker also offer only the institutions that hold one
//   of them, select the only one, and show the institution of a subject that is already chosen when it opens. Without
//   `allowedSubjectIds` neither has any effect.
const props = withDefaults(defineProps<{
  remember?: boolean; statusBySubject?: Record<string, AssignmentStatus>; activeOnly?: boolean
  allowedSubjectIds?: string[]; subjectInstitutions?: Record<string, string>
}>(), { remember: true, statusBySubject: undefined, activeOnly: false, allowedSubjectIds: undefined, subjectInstitutions: undefined })
const subjectId = defineModel<string>({ default: '' })
const pt = usePt()
const KEY = 'exameow-admin-subject'
const KIND_KEY: Record<string, PlatformKey> = { department: 'kindDepartment', level: 'kindLevel', year: 'kindYear', term: 'kindTerm' }
const STATUS_KEY: Record<AssignmentStatus, PlatformKey> = { pending: 'statusPending', approved: 'statusApproved', rejected: 'statusRejected' }

const institutions = ref<Institution[]>([])
const structure = ref<Structure | null>(null)
const institutionId = ref('')
/** chosen unit per depth ('' = all of that level) */
const unitPath = ref<string[]>([])
const error = ref('')

const allowed = computed(() => (props.allowedSubjectIds ? new Set(props.allowedSubjectIds) : null))
/** institutions that hold at least one allowed subject (null = no restriction or unknown) */
const allowedInstitutions = computed(() => {
  if (!allowed.value || !props.subjectInstitutions) return null
  return new Set([...allowed.value].map((id) => props.subjectInstitutions?.[id]).filter((x): x is string => !!x))
})
const shown = computed(() => {
  const st = structure.value
  if (!st) return null
  const base = props.activeOnly ? activeSubset(st.units, st.subjects) : { units: st.units, subjects: st.subjects }
  return allowed.value ? restrictToSubjects(base.units, base.subjects, allowed.value) : base
})
const idx = computed(() => (shown.value ? buildIndex(shown.value.units, shown.value.subjects) : null))
const shownInstitutions = computed(() => {
  const active = props.activeOnly ? institutions.value.filter((i) => i.is_active) : institutions.value
  const only = allowedInstitutions.value
  return only ? active.filter((i) => only.has(i.id)) : active
})
const suffix = (active: boolean) => (active ? '' : ` (${pt('inactive')})`)
const statusOf = (id: string): AssignmentStatus | undefined => props.statusBySubject?.[id]
const statusSuffix = (id: string) => {
  const st = statusOf(id)
  return st ? ` — ${pt(STATUS_KEY[st])}` : ''
}

interface Level { depth: number; options: Unit[]; value: string; label: string }
/** One select per depth: the children of the node chosen above (roots first), until "all" is chosen or a leaf is reached. */
const levels = computed<Level[]>(() => {
  const index = idx.value
  if (!index) return []
  const out: Level[] = []
  let parent: string | null = null
  for (let depth = 0; ; depth++) {
    const options: Unit[] = children(index, parent)
    if (!options.length) break
    const chosen: string = unitPath.value[depth] ?? ''
    const value: string = options.some((u) => u.id === chosen) ? chosen : ''
    const kinds = [...new Set(options.map((u) => u.kind))]
    out.push({ depth, options, value, label: kinds.length === 1 ? pt(KIND_KEY[kinds[0] as string] ?? 'pickLevelOrDept') : pt('pickLevelOrDept') })
    if (!value) break
    parent = value
  }
  return out
})
const selectedUnit = computed<string | null>(() => {
  const deepest = [...levels.value].reverse().find((l) => l.value)
  return deepest ? deepest.value : null
})
const subjects = computed(() => (idx.value ? subtreeSubjects(idx.value, selectedUnit.value) : []))

async function loadStructure() {
  structure.value = null
  unitPath.value = []
  if (!institutionId.value) return
  try {
    structure.value = await getStructure(institutionId.value)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

function pickOnlySubject() {
  const only = subjects.value.length === 1 ? subjects.value[0]! : null
  if (only && subjectSelectable(statusOf(only.id))) subjectId.value = only.id
}

async function onInstitution() {
  subjectId.value = ''
  error.value = ''
  await loadStructure()
  pickOnlySubject()
}

function onUnit(depth: number, value: string) {
  unitPath.value = value ? [...unitPath.value.slice(0, depth), value] : unitPath.value.slice(0, depth)
  // a subject outside the narrowed list is dropped; if only one remains it is chosen
  if (subjectId.value && !subjects.value.some((s) => s.id === subjectId.value)) subjectId.value = ''
  pickOnlySubject()
}

/** Picking a subject shows where it sits: the lists above jump to its level/department/… */
function onSubject() {
  if (idx.value) unitPath.value = pathToSubject(idx.value, subjectId.value).map((u) => u.id)
}

watch(subjectId, (v) => {
  if (!props.remember) return
  try { localStorage.setItem(KEY, JSON.stringify({ i: institutionId.value, s: v })) } catch { /* storage unavailable */ }
})

onMounted(async () => {
  try {
    institutions.value = await listInstitutions()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    return
  }
  // A subject that is already chosen (a link with ?subject=) is shown with its institution and units.
  const preset = subjectId.value
  const presetInstitution = preset ? props.subjectInstitutions?.[preset] : undefined
  if (presetInstitution && shownInstitutions.value.some((i) => i.id === presetInstitution)) {
    institutionId.value = presetInstitution
    await loadStructure()
    if (shown.value?.subjects.some((s) => s.id === preset)) onSubject()
    else subjectId.value = ''
    return
  }
  // A single possible institution is selected for the person (and a single subject in it too).
  if (allowedInstitutions.value && shownInstitutions.value.length === 1 && !institutionId.value) {
    institutionId.value = shownInstitutions.value[0]!.id
    await loadStructure()
    pickOnlySubject()
    return
  }
  let saved: { i?: string; s?: string } = {}
  if (!props.remember) return
  try { saved = JSON.parse(localStorage.getItem(KEY) ?? '{}') } catch { /* ignore */ }
  if (saved.i && shownInstitutions.value.some((i) => i.id === saved.i)) {
    institutionId.value = saved.i
    await loadStructure()
    if (saved.s && shown.value?.subjects.some((s) => s.id === saved.s)) {
      subjectId.value = saved.s
      onSubject()
    }
  }
})
</script>

<template>
  <div class="grid grid-cols-1 sm:grid-cols-2 gap-3" data-testid="subject-picker">
    <label class="block">
      <span class="text-label-lg">{{ pt('bankInstitution') }}</span>
      <select v-model="institutionId" class="input-outlined w-full mt-1" data-testid="pick-institution" @change="onInstitution">
        <option value="" disabled></option>
        <option v-for="i in shownInstitutions" :key="i.id" :value="i.id">{{ i.name_ar }}</option>
      </select>
    </label>
    <label v-for="l in levels" :key="'u' + l.depth" class="block">
      <span class="text-label-lg">{{ l.label }}</span>
      <select :value="l.value" class="input-outlined w-full mt-1" :data-testid="'pick-unit-' + l.depth" @change="onUnit(l.depth, ($event.target as HTMLSelectElement).value)">
        <option value="">{{ pt('pickAll') }}</option>
        <option v-for="u in l.options" :key="u.id" :value="u.id">{{ u.name_ar }}{{ suffix(u.is_active) }}</option>
      </select>
    </label>
    <label class="block">
      <span class="text-label-lg">{{ pt('bankSubject') }}</span>
      <select v-model="subjectId" class="input-outlined w-full mt-1" :disabled="!subjects.length" data-testid="pick-subject" @change="onSubject">
        <option value="" disabled></option>
        <option v-for="s in subjects" :key="s.id" :value="s.id" :disabled="!subjectSelectable(statusOf(s.id))" :data-testid="`pick-subject-option-${s.id}`">{{ s.name_ar }}{{ suffix(s.is_active) }}{{ statusSuffix(s.id) }}</option>
      </select>
    </label>
    <p v-if="institutionId && structure && !subjects.length && !error" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-on-surface-variant))" data-testid="pick-empty">
      {{ selectedUnit ? pt('pickNoSubjectsHere') : pt('bankNoSubjects') }}
    </p>
    <p v-if="error" role="alert" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-error))">{{ error }}</p>
  </div>
</template>
