<script setup lang="ts">
import { ref } from 'vue'
import { usePt } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'

// "Leave without saving?" - shown by an editor (see composables/useUnsavedGuard.ts) while the router waits for the answer.
// Staying is the default: focus starts on it and Escape means stay.
const emit = defineEmits<{ stay: []; leave: [] }>()
const pt = usePt()
const open = ref(true)
const panel = ref<HTMLElement | null>(null)
useDialog(open, panel, () => emit('stay'))
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.5)" data-testid="unsaved-dialog">
    <div ref="panel" class="card-elevated p-5 w-full max-w-md space-y-4" role="alertdialog" aria-modal="true" aria-labelledby="unsaved-title" aria-describedby="unsaved-msg">
      <h2 id="unsaved-title" class="text-title-lg font-bold">{{ pt('unsavedTitle') }}</h2>
      <p id="unsaved-msg" class="text-body-md">{{ pt('unsavedMsg') }}</p>
      <div class="flex flex-wrap gap-2 justify-end">
        <button type="button" class="btn-filled" data-autofocus data-testid="unsaved-stay" @click="emit('stay')">{{ pt('unsavedStay') }}</button>
        <button type="button" class="btn-outlined" data-testid="unsaved-leave" @click="emit('leave')">{{ pt('unsavedLeave') }}</button>
      </div>
    </div>
  </div>
</template>
