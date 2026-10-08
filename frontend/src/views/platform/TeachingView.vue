<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myTeaching, requestTeaching, dropTeaching, type Teaching } from '@/api/platformLearning'
import SubjectPicker from '@/components/platform/SubjectPicker.vue'

// A teacher's own subject requests, and the one place where a new request starts: pick the institution and the subject
// here and ask. It works for an active teacher and also for one whose account is still waiting for approval (the
// server lets a pending teacher browse the catalogue and manage requests, nothing else).
const pt = usePt()
const auth = useAuthStore()
const items = ref<Teaching[]>([])
const error = ref('')
const sent = ref(false)
const busy = ref(false)
const subjectId = ref('')
const statusKey: Record<string, PlatformKey> = { pending: 'statusPending', approved: 'statusApproved', rejected: 'statusRejected' }

const accountPending = computed(() => auth.profile?.status === 'pending')
// Subjects already asked for (waiting or approved) cannot be asked for again; a rejected one can.
const existing = computed(() => items.value.find((t) => t.subject_id === subjectId.value) ?? null)
const alreadyOpen = computed(() => existing.value?.status === 'pending' || existing.value?.status === 'approved')

async function load() {
  try {
    items.value = await myTeaching()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function request() {
  if (!subjectId.value || alreadyOpen.value || busy.value) return
  busy.value = true
  error.value = ''
  sent.value = false
  try {
    await requestTeaching(subjectId.value)
    sent.value = true
    subjectId.value = ''
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}

async function drop(t: Teaching) {
  if (!window.confirm(pt('confirmDelete'))) return
  error.value = ''
  sent.value = false
  try {
    await dropTeaching(t.subject_id)
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="teaching-page">
    <router-link :to="auth.isActive ? '/platform/teacher' : '/auth/pending'" class="text-body-sm underline" data-testid="teaching-back">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myTeaching') }}</h1>

    <p v-if="accountPending" class="card-filled p-3 text-body-sm mb-4" role="note" data-testid="teaching-account-pending">{{ pt('tchReqPendingAccount') }}</p>

    <section class="card-filled p-4 mb-6 space-y-3" aria-labelledby="teaching-request-title" data-testid="teaching-request-card">
      <h2 id="teaching-request-title" class="text-title-md font-bold">{{ pt('tchReqTitle') }}</h2>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('addTeachingHint') }}</p>
      <SubjectPicker v-model="subjectId" :remember="false" />
      <p v-if="alreadyOpen" class="text-body-sm" role="status" data-testid="teaching-already">{{ pt('teachingPending') }}</p>
      <p v-if="sent" class="text-body-sm" role="status" data-testid="teaching-sent">{{ pt('tchReqSent') }}</p>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="teaching-error">{{ error }}</p>
      <button type="button" class="btn-filled" :disabled="!subjectId || alreadyOpen || busy" data-testid="teaching-request" @click="request">{{ pt('requestTeaching') }}</button>
    </section>

    <section aria-labelledby="teaching-list-title">
      <h2 id="teaching-list-title" class="text-title-md font-bold mb-3">{{ pt('tchReqList') }}</h2>
      <p v-if="!items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="teaching-empty">{{ pt('noResults') }}</p>
      <ul class="space-y-3" data-testid="teaching-list">
        <li v-for="t in items" :key="t.subject_id" class="card-filled p-4 flex items-center gap-3" :data-testid="`teaching-item-${t.subject_id}`">
          <!-- a pending teacher cannot open subject pages yet, so the name is plain text for them -->
          <router-link v-if="auth.isActive" :to="`/platform/subjects/${t.subject_id}`" class="flex-1 min-w-0">
            <div class="font-bold" dir="auto">{{ t.subject_name }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" dir="auto">{{ t.institution_name }}</div>
          </router-link>
          <div v-else class="flex-1 min-w-0">
            <div class="font-bold" dir="auto">{{ t.subject_name }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" dir="auto">{{ t.institution_name }}</div>
          </div>
          <span class="px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[t.status]!) }}</span>
          <button class="btn-text" :aria-label="`${pt('del')}: ${t.subject_name}`" :data-testid="`teaching-drop-${t.subject_id}`" @click="drop(t)">{{ pt('del') }}</button>
        </li>
      </ul>
    </section>
  </div>
</template>
