<script setup lang="ts">
import { reactive, ref } from 'vue'
import { usePt } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'

// One dialog for adding/editing an institution, a level/department/… or a subject. The caller says which fields exist.
export interface StructureValues { name_ar: string; name_en: string; city: string; kind: string }
const props = defineProps<{
  title: string
  initial?: Partial<StructureValues>
  /** unit kinds to choose from (only when creating a unit) */
  kinds?: { kind: string; label: string }[]
  withCity?: boolean
  busy?: boolean
  error?: string
}>()
const emit = defineEmits<{ save: [StructureValues]; close: [] }>()
const pt = usePt()

const open = ref(true)
const panel = ref<HTMLElement | null>(null)
useDialog(open, panel, () => emit('close'))
const f = reactive<StructureValues>({
  name_ar: props.initial?.name_ar ?? '',
  name_en: props.initial?.name_en ?? '',
  city: props.initial?.city ?? '',
  kind: props.initial?.kind ?? props.kinds?.[0]?.kind ?? '',
})
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.5)" data-testid="structure-dialog">
    <form ref="panel" class="card-elevated p-5 w-full max-w-md space-y-4" role="dialog" aria-modal="true" aria-labelledby="structure-dialog-title" @submit.prevent="emit('save', { ...f })">
      <h2 id="structure-dialog-title" class="text-title-lg font-bold">{{ props.title }}</h2>
      <label v-if="props.kinds?.length" class="block">
        <span class="text-label-lg">{{ pt('stcKind') }}</span>
        <select v-model="f.kind" class="input-outlined w-full mt-1" data-testid="sd-kind">
          <option v-for="k in props.kinds" :key="k.kind" :value="k.kind">{{ k.label }}</option>
        </select>
      </label>
      <label class="block">
        <span class="text-label-lg">{{ pt('nameAr') }}</span>
        <input v-model="f.name_ar" required maxlength="200" class="input-outlined w-full mt-1" data-autofocus data-testid="sd-name-ar" />
      </label>
      <label class="block">
        <span class="text-label-lg">{{ pt('nameEn') }}</span>
        <input v-model="f.name_en" maxlength="200" dir="ltr" class="input-outlined w-full mt-1" data-testid="sd-name-en" />
      </label>
      <label v-if="props.withCity" class="block">
        <span class="text-label-lg">{{ pt('city') }}</span>
        <input v-model="f.city" maxlength="100" class="input-outlined w-full mt-1" data-testid="sd-city" />
      </label>
      <p v-if="props.error" role="alert" class="text-body-sm font-semibold" style="color: rgb(var(--md-error))" data-testid="sd-error">{{ props.error }}</p>
      <div class="flex gap-2 justify-end">
        <button type="button" class="btn-outlined" @click="emit('close')">{{ pt('cancel') }}</button>
        <button type="submit" class="btn-filled" :disabled="props.busy || !f.name_ar.trim()" data-testid="sd-save">{{ pt('save') }}</button>
      </div>
    </form>
  </div>
</template>
