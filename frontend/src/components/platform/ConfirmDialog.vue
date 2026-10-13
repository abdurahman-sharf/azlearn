<script setup lang="ts">
import { ref } from 'vue'
import { usePt } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'

// A plain "are you sure?" for an action that deletes nothing (e.g. enrolling in several subjects at once): the same
// keyboard behaviour as every dialog (focus inside, Tab cycles, Escape cancels, focus goes back), a neutral confirm
// button, and an optional list of what the action applies to. Deleting uses ConfirmDeleteDialog instead.
const props = withDefaults(defineProps<{
  title: string
  message: string
  confirmLabel: string
  /** what the action applies to (shown as a list; user text, so it is rendered as plain text) */
  items?: string[]
  busy?: boolean
  error?: string
  /** prefix of the test ids: `<testid>`, `<testid>-yes`, `<testid>-no` */
  testid?: string
  /** the cancel button's text when "Cancel" is not the right word (e.g. "Stay in the exam") */
  cancelLabel?: string
}>(), { testid: 'confirm-dialog' })
const emit = defineEmits<{ confirm: []; close: [] }>()
const pt = usePt()

const open = ref(true)
const panel = ref<HTMLElement | null>(null)
useDialog(open, panel, () => emit('close'))
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.5)" :data-testid="props.testid">
    <div
      ref="panel"
      class="card-elevated p-5 w-full max-w-md max-h-[90vh] overflow-y-auto"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="props.testid + '-title'"
      :aria-describedby="props.testid + '-msg'"
    >
      <form class="space-y-4" @submit.prevent="!props.busy && emit('confirm')">
        <h2 :id="props.testid + '-title'" class="text-title-lg font-bold">{{ props.title }}</h2>
        <p :id="props.testid + '-msg'" class="text-body-md">{{ props.message }}</p>
        <ul v-if="props.items?.length" class="list-disc ps-5 text-body-md space-y-0.5" :data-testid="props.testid + '-items'">
          <li v-for="(it, i) in props.items" :key="i" dir="auto" class="break-words">{{ it }}</li>
        </ul>
        <!-- extra content of the dialog (a choice, a summary): its form controls take part in the dialog's focus cycle -->
        <slot />
        <p v-if="props.error" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))">{{ props.error }}</p>
        <div class="flex flex-wrap gap-2 justify-end">
          <button type="button" class="btn-outlined" :data-testid="props.testid + '-no'" @click="emit('close')">{{ props.cancelLabel ?? pt('cancel') }}</button>
          <button type="submit" class="btn-filled" data-autofocus :disabled="props.busy" :data-testid="props.testid + '-yes'">{{ props.confirmLabel }}</button>
        </div>
      </form>
    </div>
  </div>
</template>
