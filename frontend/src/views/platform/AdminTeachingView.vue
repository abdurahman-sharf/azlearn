<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { adminTeaching, decideTeaching, type Teaching } from '@/api/platformLearning'

const pt = usePt()
const items = ref<Teaching[]>([])
const error = ref('')
const accountKey: Record<string, PlatformKey> = { active: 'statusActive', pending: 'statusPending', rejected: 'statusRejected', suspended: 'statusSuspended' }

async function load() {
  try {
    items.value = await adminTeaching('pending')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function decide(t: Teaching, status: 'approved' | 'rejected') {
  error.value = ''
  try {
    await decideTeaching({ teacher_id: t.teacher_id, subject_id: t.subject_id, status })
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminTeaching') }}</h1>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noRequests') }}</p>
    <ul class="space-y-3">
      <li v-for="t in items" :key="t.teacher_id + t.subject_id" class="card-filled p-4 space-y-2">
        <div>
          <span class="text-body-sm">{{ pt('teacher') }}:</span> <router-link :to="`/platform/teachers/${t.teacher_id}`" class="font-bold underline" dir="auto">{{ t.teacher_name }}</router-link>
          <!-- a request can come from an account that is still waiting for its own approval; approving the subject does not activate it -->
          <span v-if="t.teacher_status && t.teacher_status !== 'active'" class="ms-2 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" data-testid="teacher-account-status">{{ pt('teacherAccountStatus') }}: {{ pt(accountKey[t.teacher_status] ?? 'statusPending') }}</span>
        </div>
        <div><span class="text-body-sm">{{ pt('subject') }}:</span> <span class="font-bold">{{ t.subject_name }}</span> <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">— {{ t.institution_name }}</span></div>
        <div class="flex gap-2">
          <button class="btn-filled" @click="decide(t, 'approved')">{{ pt('approve') }}</button>
          <button class="btn-outlined" @click="decide(t, 'rejected')">{{ pt('reject') }}</button>
        </div>
      </li>
    </ul>
  </div>
</template>
