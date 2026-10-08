<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { getStructure, type Structure, type Unit, type UnitKind } from '@/api/platformAdmin'
import { getPlacement, setPlacement, type Placement } from '@/api/platformLearning'
import PageError from '@/components/platform/PageError.vue'

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const id = route.params.id as string

const data = ref<Structure | null>(null)
const placement = ref<Placement | null>(null)
const error = ref('')

const kindKey: Record<UnitKind, PlatformKey> = { department: 'kindDepartment', level: 'kindLevel', year: 'kindYear', term: 'kindTerm' }

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

async function load() {
  try {
    data.value = await getStructure(id)
    if (auth.role === 'student') placement.value = await getPlacement()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function chooseLevel(u: Unit) {
  try {
    const p = { institution_id: id, unit_id: u.id }
    await setPlacement(p)
    placement.value = p
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

onMounted(load)
</script>

<template>
  <div v-if="data" class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3">{{ data.institution.name_ar }}</h1>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">
      {{ [data.institution.name_en, data.institution.city].filter(Boolean).join(' · ') }}
    </p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <p v-if="!rows.length && !subjectsOf(null).length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>

    <ul class="space-y-2">
      <li v-for="{ unit, depth } in rows" :key="unit.id">
        <div class="card-filled p-3" :style="{ marginInlineStart: `${depth * 1.25}rem` }">
          <div class="flex items-center gap-2">
            <span class="shrink-0 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-secondary-container))">{{ pt(kindKey[unit.kind]) }}</span>
            <span class="font-bold break-words min-w-0 flex-1">{{ unit.name_ar }}</span>
            <span v-if="placement?.unit_id === unit.id" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary-container))">{{ pt('myLevelSet') }}</span>
            <button v-else-if="auth.role === 'student' && unit.kind !== 'department'" class="btn-text" @click="chooseLevel(unit)">{{ pt('myLevel') }}</button>
          </div>
          <div v-if="subjectsOf(unit.id).length" class="flex flex-wrap gap-2 mt-2">
            <router-link v-for="s in subjectsOf(unit.id)" :key="s.id" :to="`/platform/subjects/${s.id}`" class="px-3 py-1 rounded-full text-sm" style="background-color: rgb(var(--md-surface-container-high))">{{ s.name_ar }}</router-link>
          </div>
        </div>
      </li>
    </ul>

    <div v-if="subjectsOf(null).length" class="mt-5">
      <div class="font-semibold mb-2">{{ pt('subjects') }}</div>
      <div class="flex flex-wrap gap-2">
        <router-link v-for="s in subjectsOf(null)" :key="s.id" :to="`/platform/subjects/${s.id}`" class="px-3 py-1 rounded-full text-sm" style="background-color: rgb(var(--md-surface-container-high))">{{ s.name_ar }}</router-link>
      </div>
    </div>
  </div>
  <PageError v-else-if="error" :message="error" />
</template>
