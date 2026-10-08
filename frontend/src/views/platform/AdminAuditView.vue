<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, auditLabel } from '@/i18n/platform'
import { adminAudit, type AuditRow } from '@/api/platformOps'

const pt = usePt()
const i18n = useI18nStore()
const rows = ref<AuditRow[]>([])
const more = ref(false)
const error = ref('')
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load(before?: number) {
  try {
    const page = await adminAudit(before)
    rows.value = before ? [...rows.value, ...page] : page
    more.value = page.length === 50
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(() => load())
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminAudit') }}</h1>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!rows.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-2">
      <li v-for="r in rows" :key="r.id" class="card-filled p-3" data-testid="audit-row">
        <div class="font-bold">{{ auditLabel(pt, r.action) }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
          {{ pt('actor') }}: {{ r.actor ?? pt('system') }} · {{ fmt(r.created_at) }}
          <template v-if="r.detail"> · <span dir="auto">{{ r.detail }}</span></template>
        </div>
      </li>
    </ul>
    <button v-if="more" class="btn-outlined mt-4" @click="load(rows[rows.length - 1]!.id)">{{ pt('loadMore') }}</button>
  </div>
</template>
