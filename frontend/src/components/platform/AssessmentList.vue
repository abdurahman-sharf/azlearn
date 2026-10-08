<script setup lang="ts">
import { usePt } from '@/i18n/platform'
import type { AssessmentInfo } from '@/api/platformExams'
import OwnerStateChip from './OwnerStateChip.vue'
import HiddenWhy from './HiddenWhy.vue'

// `own` is the teacher's view of their own exams (My content): no "by <me>", and one state chip that says "hidden:
// <reason>" instead of a plain "published" when students cannot see the exam.
defineProps<{ items: AssessmentInfo[]; showStatus?: boolean; showEdit?: boolean; own?: boolean }>()
const pt = usePt()
</script>

<template>
  <ul v-if="items.length" class="space-y-2">
    <li v-for="a in items" :key="a.id" class="card-filled p-3" :data-testid="`content-exam-${a.id}`">
      <div class="flex flex-wrap items-center gap-2">
        <router-link :to="`/platform/assessments/${a.id}`" class="flex-1 min-w-[9rem]">
          <div class="font-bold break-words" dir="auto">{{ a.title }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            <span dir="ltr" class="inline-block">{{ a.question_count }}</span> {{ pt('questionsCount') }} · {{ a.subject_name }}<template v-if="!own"> · {{ pt('by') }} {{ a.teacher_name }}</template>
            <template v-if="a.duration_min"> · <span dir="ltr" class="inline-block">{{ a.duration_min }}</span> {{ pt('minutesShort') }}</template>
          </div>
        </router-link>
        <OwnerStateChip v-if="own" :row="{ status: a.status, visible: a.visible, hidden_reason: a.hidden_reason }" />
        <span v-else-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ a.status === 'published' ? pt('statusPublished') : a.status === 'closed' ? pt('exPhase_closed') : a.status === 'archived' ? pt('exPhase_archived') : pt('statusDraft') }}</span>
        <router-link v-if="own && showEdit && a.status !== 'archived'" :to="`/platform/assessments/${a.id}/edit`" class="btn-text" :aria-label="`${pt('edit')}: ${a.title}`" :data-testid="`content-exam-edit-${a.id}`">{{ pt('edit') }}</router-link>
        <router-link v-if="showEdit" :to="`/platform/assessments/${a.id}/results`" class="btn-text" :aria-label="`${pt('results')}: ${a.title}`">{{ pt('results') }}</router-link>
      </div>
      <HiddenWhy v-if="own && (a.status === 'published' || a.status === 'closed')" :reason="a.hidden_reason" class="mt-1" />
    </li>
  </ul>
</template>
