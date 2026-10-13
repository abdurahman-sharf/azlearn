<script setup lang="ts">
import { computed } from 'vue'
import { usePt } from '@/i18n/platform'
import { FEEDBACK_MAX, charCount, cleanFeedback, feedbackTooLong, pointsProblem, type GradeCell } from '@/utils/gradeSheet'

// The boxes for grading ONE written answer: points (with "full" / "zero" shortcuts) and a comment for the student.
// Shared by the grading sheet (one per student) and the attempt page (one per question), which is why the accessible
// names come from `label` - the student and/or question this row belongs to - and not from the position on the page.
const props = defineProps<{
  /** unique prefix of the element ids on the page (an attempt id or a question id) */
  idPrefix: string
  /** full marks of the question */
  max: number
  modelValue: GradeCell
  /** names the row for assistive technology, e.g. "Sara - question 2" */
  label: string
  pointsTestid: string
  feedbackTestid: string
}>()
const emit = defineEmits<{ 'update:modelValue': [GradeCell] }>()
const pt = usePt()

const pointsId = computed(() => `${props.idPrefix}-points`)
const feedbackId = computed(() => `${props.idPrefix}-feedback`)
const pointsHelpId = computed(() => `${props.idPrefix}-points-help`)
const problemId = computed(() => `${props.idPrefix}-points-problem`)
const countId = computed(() => `${props.idPrefix}-count`)
const problem = computed(() => pointsProblem(props.modelValue.points, props.max))
const length = computed(() => charCount(cleanFeedback(props.modelValue.feedback)))
const tooLong = computed(() => feedbackTooLong(props.modelValue.feedback))

const set = (patch: Partial<GradeCell>) => emit('update:modelValue', { ...props.modelValue, ...patch })

/** Enter in a points box moves to the next one (grading a column of answers without the mouse); Ctrl/Cmd+Enter is left for "save". */
function onEnter(ev: KeyboardEvent) {
  if (ev.ctrlKey || ev.metaKey) return
  ev.preventDefault()
  const inputs = [...document.querySelectorAll<HTMLInputElement>('[data-grade-input]')]
  inputs[inputs.indexOf(ev.target as HTMLInputElement) + 1]?.focus()
}
</script>

<template>
  <div class="space-y-2">
    <div class="flex items-center gap-2 flex-wrap">
      <label :for="pointsId" class="flex items-center gap-2 text-body-sm">
        <span>{{ pt('exScore') }}</span>
        <span class="sr-only">{{ label }}</span>
      </label>
      <input
        :id="pointsId" :value="modelValue.points" type="number" min="0" :max="max" step="0.5" dir="ltr"
        class="input-outlined w-24 !py-1" data-grade-input :data-testid="pointsTestid"
        :aria-invalid="problem ? 'true' : undefined" :aria-describedby="problem ? `${pointsHelpId} ${problemId}` : pointsHelpId"
        @input="set({ points: ($event.target as HTMLInputElement).value })" @keydown.enter="onEnter"
      />
      <span :id="pointsHelpId" class="text-body-sm"><span class="sr-only">{{ pt('gdMax') }}: </span><span dir="ltr" class="inline-block">/ {{ max }}</span></span>
      <button type="button" class="btn-text" :aria-label="`${pt('gdFull')}: ${label}`" :data-testid="`${pointsTestid}-full`" @click="set({ points: String(max) })">{{ pt('gdFull') }}</button>
      <button type="button" class="btn-text" :aria-label="`${pt('gdZero')}: ${label}`" :data-testid="`${pointsTestid}-zero`" @click="set({ points: '0' })">{{ pt('gdZero') }}</button>
    </div>
    <p v-if="problem" :id="problemId" class="text-body-sm" style="color: rgb(var(--md-error))" :data-testid="`${pointsTestid}-problem`">{{ pt('gdBadPoints') }}</p>

    <div>
      <label :for="feedbackId" class="block text-body-sm">
        {{ pt('gdFeedback') }}<span class="sr-only">: {{ label }}</span>
      </label>
      <textarea
        :id="feedbackId" :value="modelValue.feedback" rows="2" dir="auto" class="input-outlined w-full mt-1"
        :data-testid="feedbackTestid" :aria-invalid="tooLong ? 'true' : undefined" :aria-describedby="countId"
        @input="set({ feedback: ($event.target as HTMLTextAreaElement).value })"
      ></textarea>
      <p :id="countId" class="text-body-sm mt-1" :data-testid="`${feedbackTestid}-count`">
        <span dir="ltr" class="inline-block">{{ length }} / {{ FEEDBACK_MAX }}</span>
        <span v-if="tooLong" class="font-semibold ms-2" style="color: rgb(var(--md-error))" :data-testid="`${feedbackTestid}-problem`">{{ pt('gdFeedbackTooLong') }}</span>
      </p>
    </div>
  </div>
</template>
