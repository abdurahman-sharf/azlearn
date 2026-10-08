<script setup lang="ts">
import { computed, ref } from 'vue'
import { usePt } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'

// Delete confirmation that says exactly what goes away. When `confirmName` is given (the item holds exam attempts)
// the user must type it first, like deleting an exam that has attempts. `confirmKind="email"` is for typing an e-mail
// address (account deletion): its own prompt, left-to-right, and case does not matter.
const props = defineProps<{
  title: string
  message: string
  counts?: { label: string; value: number }[]
  /** what survives the deletion, e.g. the subjects of a deleted level */
  kept?: string
  confirmName?: string
  confirmKind?: 'name' | 'email'
  busy?: boolean
  error?: string
}>()
const emit = defineEmits<{ confirm: []; close: [] }>()
const pt = usePt()

const open = ref(true)
const panel = ref<HTMLElement | null>(null)
useDialog(open, panel, () => emit('close'))
const typed = ref('')
const isEmail = computed(() => props.confirmKind === 'email')
const norm = (v: string) => (isEmail.value ? v.trim().toLowerCase() : v.trim())
const allowed = computed(() => !props.confirmName || norm(typed.value) === norm(props.confirmName))
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.5)" data-testid="confirm-delete">
    <form ref="panel" class="card-elevated p-5 w-full max-w-md space-y-4" role="alertdialog" aria-modal="true" aria-labelledby="confirm-delete-title" aria-describedby="confirm-delete-msg" @submit.prevent="allowed && emit('confirm')">
      <h2 id="confirm-delete-title" class="text-title-lg font-bold">{{ props.title }}</h2>
      <p id="confirm-delete-msg" class="text-body-md">{{ props.message }}</p>
      <ul v-if="props.counts?.some((c) => c.value > 0)" class="grid grid-cols-2 gap-2" data-testid="confirm-counts">
        <li v-for="c in props.counts.filter((x) => x.value > 0)" :key="c.label" class="rounded-xl px-3 py-2" style="background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))">
          <b dir="ltr" class="inline-block">{{ c.value }}</b> {{ c.label }}
        </li>
      </ul>
      <p v-if="props.kept" class="text-body-sm" style="color: rgb(var(--md-on-surface))" data-testid="confirm-kept">{{ props.kept }}</p>
      <label v-if="props.confirmName" class="block">
        <span class="text-label-lg">{{ isEmail ? pt('delTypeEmail') : pt('stcTypeName') }} <b :dir="isEmail ? 'ltr' : 'auto'" class="inline-block">{{ props.confirmName }}</b></span>
        <input v-model="typed" class="input-outlined w-full mt-1" :dir="isEmail ? 'ltr' : undefined" autocomplete="off" data-autofocus data-testid="confirm-name" />
      </label>
      <p v-if="props.error" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))">{{ props.error }}</p>
      <div class="flex gap-2 justify-end">
        <button type="button" class="btn-outlined" :data-autofocus="props.confirmName ? undefined : ''" @click="emit('close')">{{ pt('cancel') }}</button>
        <button type="submit" class="btn-filled" :disabled="props.busy || !allowed" style="background-color: rgb(var(--md-error)); color: rgb(var(--md-on-error))" data-testid="confirm-yes">{{ pt('del') }}</button>
      </div>
    </form>
  </div>
</template>
