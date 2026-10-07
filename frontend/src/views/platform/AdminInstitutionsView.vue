<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import type { InstitutionType } from '@/stores/auth'
import { listInstitutions, createInstitution, updateInstitution, deleteInstitution, type Institution } from '@/api/platformAdmin'

const pt = usePt()
const type = ref<InstitutionType>('school')
const items = ref<Institution[]>([])
const error = ref('')
const nameAr = ref('')
const nameEn = ref('')
const city = ref('')

const types: { value: InstitutionType; key: PlatformKey }[] = [
  { value: 'school', key: 'typeSchool' },
  { value: 'institute', key: 'typeInstitute' },
  { value: 'university', key: 'typeUniversity' },
]

async function load() {
  error.value = ''
  try {
    items.value = await listInstitutions(type.value)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function add() {
  error.value = ''
  try {
    await createInstitution({ type: type.value, name_ar: nameAr.value, name_en: nameEn.value || undefined, city: city.value || undefined })
    nameAr.value = nameEn.value = city.value = ''
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function toggle(i: Institution) {
  try {
    await updateInstitution(i.id, { is_active: !i.is_active })
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove(i: Institution) {
  if (!window.confirm(pt('confirmDelete'))) return
  try {
    await deleteInstitution(i.id)
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

function pick(t: InstitutionType) {
  type.value = t
  load()
}

onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminInstitutions') }}</h1>

    <div class="flex gap-2 mb-4">
      <button v-for="t in types" :key="t.value" :class="type === t.value ? 'btn-filled' : 'btn-outlined'" class="flex-1" @click="pick(t.value)">{{ pt(t.key) }}</button>
    </div>

    <form class="card-elevated p-4 mb-4 space-y-3" @submit.prevent="add">
      <div class="font-semibold">{{ pt('newInstitution') }}</div>
      <input v-model="nameAr" required maxlength="200" :placeholder="pt('nameAr')" class="input-outlined w-full" />
      <input v-model="nameEn" maxlength="200" :placeholder="pt('nameEn')" dir="ltr" class="input-outlined w-full" />
      <input v-model="city" maxlength="100" :placeholder="pt('city')" class="input-outlined w-full" />
      <button type="submit" class="btn-filled">{{ pt('add') }}</button>
    </form>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noInstitutions') }}</p>

    <ul class="space-y-3">
      <li v-for="i in items" :key="i.id" class="card-filled p-4 flex items-center gap-3" :class="{ 'opacity-60': !i.is_active }">
        <router-link :to="`/platform/admin/institutions/${i.id}`" class="flex-1 min-w-0">
          <div class="font-bold truncate">{{ i.name_ar }}</div>
          <div class="text-body-sm truncate" style="color: rgb(var(--md-on-surface-variant))">
            {{ [i.name_en, i.city].filter(Boolean).join(' · ') }}<template v-if="!i.is_active"> · {{ pt('inactive') }}</template>
          </div>
        </router-link>
        <button class="btn-text" @click="toggle(i)">{{ pt('toggleActive') }}</button>
        <button class="btn-text" @click="remove(i)">{{ pt('del') }}</button>
      </li>
    </ul>
  </div>
</template>
