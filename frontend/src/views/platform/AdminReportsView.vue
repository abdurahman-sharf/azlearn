<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { adminReports, resolveReport, type ReportRow } from '@/api/platformOps'

const pt = usePt()
const tab = ref<'open' | 'closed'>('open')
const rows = ref<ReportRow[]>([])
const error = ref('')

async function load() {
  error.value = ''
  try {
    if (tab.value === 'open') rows.value = await adminReports('open')
    else rows.value = [...(await adminReports('resolved')), ...(await adminReports('dismissed'))].sort((a, b) => b.created_at - a.created_at)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function close(r: ReportRow, status: 'resolved' | 'dismissed') {
  const note = window.prompt(pt('note'))
  if (note === null) return
  try {
    await resolveReport(r.id, { status, note: note || undefined })
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
function pick(t: 'open' | 'closed') { tab.value = t; load() }
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminReports') }}</h1>
    <div class="flex gap-2 mb-4">
      <button :class="tab === 'open' ? 'btn-filled' : 'btn-outlined'" @click="pick('open')">{{ pt('openReports') }}</button>
      <button :class="tab === 'closed' ? 'btn-filled' : 'btn-outlined'" @click="pick('closed')">{{ pt('closedReports') }}</button>
    </div>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!rows.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-3">
      <li v-for="r in rows" :key="r.id" class="card-filled p-4 space-y-2" data-testid="report">
        <div class="flex items-start gap-2">
          <div class="flex-1 min-w-0">
            <div class="font-bold break-words">{{ r.title || r.target_type }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ r.target_type }} · {{ pt('reportedBy') }}: {{ r.reporter }}</div>
          </div>
          <span v-if="r.status === 'open'" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-secondary-container))">{{ r.open_reports_on_target }} {{ pt('reportsOnTarget') }}</span>
        </div>
        <!-- plain text: reasons are user input -->
        <p class="whitespace-pre-wrap break-words" dir="auto">{{ r.reason }}</p>
        <p v-if="r.note" class="text-body-sm">{{ pt('note').replace(/\s*\(.*\)/, '') }}: {{ r.note }}</p>
        <div class="flex flex-wrap gap-2">
          <router-link v-if="r.link" :to="r.link" class="btn-tonal">{{ pt('openTarget') }}</router-link>
          <template v-if="r.status === 'open'">
            <button class="btn-filled" data-testid="resolve" @click="close(r, 'resolved')">{{ pt('resolve') }}</button>
            <button class="btn-outlined" data-testid="dismiss" @click="close(r, 'dismissed')">{{ pt('dismiss') }}</button>
          </template>
        </div>
      </li>
    </ul>
  </div>
</template>
