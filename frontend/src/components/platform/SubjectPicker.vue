<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { getStructure, listInstitutions, type Institution, type Subject } from '@/api/platformAdmin'

// Institution → subject picker for admin screens; the last choice is remembered per browser.
const subjectId = defineModel<string>({ default: '' })
const pt = usePt()
const KEY = 'exameow-admin-subject'

const institutions = ref<Institution[]>([])
const subjects = ref<Subject[]>([])
const institutionId = ref('')
const error = ref('')

async function loadSubjects() {
  subjects.value = []
  if (!institutionId.value) return
  try {
    subjects.value = (await getStructure(institutionId.value)).subjects
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function onInstitution() {
  subjectId.value = ''
  await loadSubjects()
  if (subjects.value.length === 1) subjectId.value = subjects.value[0]!.id
}

watch(subjectId, (v) => {
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
  try { saved = JSON.parse(localStorage.getItem(KEY) ?? '{}') } catch { /* ignore */ }
  if (saved.i && institutions.value.some((i) => i.id === saved.i)) {
    institutionId.value = saved.i
    await loadSubjects()
    if (saved.s && subjects.value.some((s) => s.id === saved.s)) subjectId.value = saved.s
  }
})
</script>

<template>
  <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
    <label class="block">
      <span class="text-label-lg">{{ pt('bankInstitution') }}</span>
      <select v-model="institutionId" class="input-outlined w-full mt-1" data-testid="pick-institution" @change="onInstitution">
        <option value="" disabled></option>
        <option v-for="i in institutions" :key="i.id" :value="i.id">{{ i.name_ar }}</option>
      </select>
    </label>
    <label class="block">
      <span class="text-label-lg">{{ pt('bankSubject') }}</span>
      <select v-model="subjectId" class="input-outlined w-full mt-1" :disabled="!subjects.length" data-testid="pick-subject">
        <option value="" disabled></option>
        <option v-for="s in subjects" :key="s.id" :value="s.id">{{ s.name_ar }}</option>
      </select>
    </label>
    <p v-if="institutionId && !subjects.length && !error" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-on-surface-variant))">{{ pt('bankNoSubjects') }}</p>
    <p v-if="error" role="alert" class="text-body-sm sm:col-span-2" style="color: rgb(var(--md-error))">{{ error }}</p>
  </div>
</template>
