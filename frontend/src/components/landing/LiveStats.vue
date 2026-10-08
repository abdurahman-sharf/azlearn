<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { ClipboardDocumentCheckIcon, QuestionMarkCircleIcon, BookOpenIcon } from '@heroicons/vue/24/outline'
import { usePt } from '@/i18n/platform'
import { usePublicStats } from '@/composables/usePublicStats'
import StatCounter from './StatCounter.vue'

// The platform's live numbers (questions, subjects, exams taken). Hidden until there is something to show: a band of
// zeros — or an error for a decorative strip — would only hurt the first impression.
const pt = usePt()
const { stats, load } = usePublicStats()
onMounted(load)
const visible = computed(() => !!stats.value && (stats.value.questions > 0 || stats.value.subjects > 0 || stats.value.attempts > 0))
</script>

<template>
  <section v-if="visible && stats" class="rounded-3xl px-4 py-8 sm:py-10" style="background-color: rgb(var(--azl-deep-blue))" :aria-label="pt('lpStatsTitle')" data-testid="live-stats">
    <ul class="grid grid-cols-1 sm:grid-cols-3 gap-8 sm:gap-4">
      <StatCounter :value="stats.questions" :label="pt('lpStatQuestions')" :icon="QuestionMarkCircleIcon" />
      <StatCounter :value="stats.subjects" :label="pt('lpStatSubjects')" :icon="BookOpenIcon" />
      <StatCounter :value="stats.attempts" :label="pt('lpStatAttempts')" :icon="ClipboardDocumentCheckIcon" />
    </ul>
  </section>
</template>
