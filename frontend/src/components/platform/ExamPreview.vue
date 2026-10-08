<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Question } from '@exameow/shared'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { letter, totals } from '@/utils/examBuilder'
import { useDialog } from '@/composables/useDialog'

// Shows the exam the way a student would see it before starting: no correct answers, nothing is created.
const props = defineProps<{
  title: string
  description: string
  questions: Question[]
  durationMin: number | null
  maxAttempts: number
  shuffle: boolean
}>()
const emit = defineEmits<{ close: [] }>()
const panel = ref<HTMLElement | null>(null)
useDialog(ref(true), panel, () => emit('close'))
const pt = usePt()
const sum = computed(() => totals(props.questions))
const options = (q: Question) => (q.type === 'true_false' && !q.options.length ? ['صحيح', 'خطأ'] : q.options)
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto p-3 sm:p-6" style="background-color: rgb(0 0 0 / 0.5)" data-testid="exam-preview" role="dialog" aria-modal="true" :aria-label="pt('exPreviewTitle')">
    <div ref="panel" class="w-full max-w-2xl rounded-2xl p-5 space-y-4 my-4" style="background-color: rgb(var(--md-surface)); color: rgb(var(--md-on-surface))">
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('exPreviewNote') }}</p>
          <h2 class="text-title-lg font-bold break-words" dir="auto" data-testid="pv-title">{{ title }}</h2>
          <p v-if="description" class="text-body-md whitespace-pre-wrap break-words" dir="auto">{{ description }}</p>
        </div>
        <button class="btn-outlined shrink-0" data-testid="pv-close" @click="emit('close')">{{ pt('exClosePreview') }}</button>
      </div>
      <dl class="grid grid-cols-3 gap-2 text-center">
        <div class="card-elevated p-2"><dt class="text-body-sm">{{ pt('exQuestionCount') }}</dt><dd class="font-bold" dir="ltr">{{ questions.length }}</dd></div>
        <div class="card-elevated p-2"><dt class="text-body-sm">{{ pt('exPreviewDuration') }}</dt><dd class="font-bold" dir="ltr">{{ durationMin ? durationMin + ' ' + pt('minutesShort') : pt('exPreviewNoLimit') }}</dd></div>
        <div class="card-elevated p-2"><dt class="text-body-sm">{{ pt('exPreviewAttempts') }}</dt><dd class="font-bold" dir="ltr">{{ maxAttempts }}</dd></div>
      </dl>
      <p v-if="shuffle" class="text-body-sm" data-testid="pv-shuffle">{{ pt('exPreviewShuffle') }}</p>
      <ol class="space-y-4">
        <li v-for="(q, n) in questions" :key="q.id" class="card-filled p-4 space-y-2" data-testid="pv-question">
          <div class="flex gap-2 items-start">
            <span dir="ltr" class="inline-block font-bold shrink-0">{{ n + 1 }}.</span>
            <div class="flex-1 min-w-0 whitespace-pre-wrap break-words font-semibold" dir="auto">{{ q.stem }}</div>
            <span class="text-body-sm shrink-0" dir="ltr">{{ q.score ?? 1 }} {{ pt('exPoints') }}</span>
          </div>
          <ol v-if="options(q).length" class="space-y-1">
            <li v-for="(o, i) in options(q)" :key="i" class="flex gap-2">
              <span dir="ltr" class="inline-block w-6 shrink-0">{{ letter(i) }}.</span><span class="flex-1 break-words" dir="auto">{{ o }}</span>
            </li>
          </ol>
          <p v-else class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt((q.type === 'short_answer' ? 'bank_t_short_answer' : 'bank_t_fill_blank') as PlatformKey) }}</p>
        </li>
      </ol>
      <p class="text-body-md font-bold">{{ pt('exTotalPoints') }}: <span dir="ltr" class="inline-block">{{ sum.points }}</span></p>
    </div>
  </div>
</template>
