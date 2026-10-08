<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { getStructure, listInstitutions, type Institution, type Structure, type Unit } from '@/api/platformAdmin'
import { buildIndex, children, pathToSubject, subtreeSubjects } from '@/utils/structureTree'

// Institution → level/department/year/term (one list per depth, as deep as the structure goes) → subject, for admin
// screens. The model is the subject id; the last subject is remembered per browser (`remember`).
const props = withDefaults(defineProps<{ remember?: boolean }>(), { remember: true })
const subjectId = defineModel<string>({ default: '' })
const pt = usePt()
const KEY = 'exameow-admin-subject'
const KIND_KEY: Record<string, PlatformKey> = { department: 'kindDepartment', level: 'kindLevel', year: 'kindYear', term: 'kindTerm' }

const institutions = ref<Institution[]>([])
const structure = ref<Structure | null>(null)
const institutionId = ref('')
/** chosen unit per depth ('' = all of that level) */
const unitPath = ref<string[]>([])
const error = ref('')

const idx = computed(() => (structure.value ? buildIndex(structure.value.units, structure.value.subjects) : null))
const suffix = (active: boolean) => (active ? '' : ` (${pt('inactive')})`)

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
  if (subjects.value.length === 1) subjectId.value = subjects.value[0]!.id
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
  let saved: { i?: string; s?: string } = {}
  if (!props.remember) return
  try { saved = JSON.parse(localStorage.getItem(KEY) ?? '{}') } catch { /* ignore */ }
  if (saved.i && institutions.value.some((i) => i.id === saved.i)) {
    institutionId.value = saved.i
    await loadStructure()
    if (saved.s && structure.value?.subjects.some((s) => s.id === saved.s)) {
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
        <option v-for="i in institutions" :key="i.id" :value="i.id">{{ i.name_ar }}</option>
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
        <option v-for="s in subjects" :key="s.id" :value="s.id">{{ s.name_ar }}{{ suffix(s.is_active) }}</option>
      </select>
    </label>
    <p v-if="institutionId && structure && !subjects.length && !error" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-on-surface-variant))" data-testid="pick-empty">
      {{ selectedUnit ? pt('pickNoSubjectsHere') : pt('bankNoSubjects') }}
    </p>
    <p v-if="error" role="alert" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-error))">{{ error }}</p>
  </div>
</template>
