<script setup lang="ts">
import { ref } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'
import { report, type ReportTarget } from '@/api/platformOps'

const props = defineProps<{ targetType: ReportTarget; targetId: string }>()
const pt = usePt()
const open = ref(false)
const formEl = ref<HTMLElement | null>(null)
useDialog(open, formEl, () => { open.value = false })
const reason = ref('')
const error = ref('')
const sent = ref(false)

async function send() {
  error.value = ''
  try {
    await report({ target_type: props.targetType, target_id: props.targetId, reason: reason.value })
    sent.value = true
    reason.value = ''
    setTimeout(() => { open.value = false; sent.value = false }, 1500)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <button class="btn-text" data-testid="report-open" @click="open = true">{{ pt('report') }}</button>
  <div v-if="open" class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.4)" role="dialog" aria-modal="true" :aria-label="pt('reportTitle')">
    <form ref="formEl" class="card-elevated p-5 max-w-sm w-full space-y-3" @submit.prevent="send">
      <h2 class="font-bold">{{ pt('reportTitle') }}</h2>
      <textarea v-model="reason" required maxlength="500" rows="4" :placeholder="pt('reportReason')" class="input-outlined w-full" data-testid="report-reason"></textarea>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="sent" class="text-body-sm" role="status">{{ pt('reportSent') }}</p>
      <div class="flex gap-2">
        <button type="submit" class="btn-filled" :disabled="sent" data-testid="report-send">{{ pt('report') }}</button>
        <button type="button" class="btn-outlined" @click="open = false">{{ pt('cancel') }}</button>
      </div>
    </form>
  </div>
</template>
