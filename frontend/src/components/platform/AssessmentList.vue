<script setup lang="ts">
import { usePt } from '@/i18n/platform'
import type { AssessmentInfo } from '@/api/platformExams'

defineProps<{ items: AssessmentInfo[]; showStatus?: boolean; showEdit?: boolean }>()
const pt = usePt()
</script>

<template>
  <ul v-if="items.length" class="space-y-2">
    <li v-for="a in items" :key="a.id" class="card-filled p-3 flex items-center gap-2">
      <router-link :to="`/platform/assessments/${a.id}`" class="flex-1 min-w-0">
        <div class="font-bold break-words">{{ a.title }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
          {{ a.question_count }} {{ pt('questionsCount') }} · {{ a.subject_name }} · {{ pt('by') }} {{ a.teacher_name }}
          <template v-if="a.duration_min"> · {{ a.duration_min }} {{ pt('minutesShort') }}</template>
        </div>
      </router-link>
      <span v-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ a.status === 'published' ? pt('statusPublished') : a.status === 'closed' ? pt('exPhase_closed') : a.status === 'archived' ? pt('exPhase_archived') : pt('statusDraft') }}</span>
      <router-link v-if="showEdit" :to="`/platform/assessments/${a.id}/results`" class="btn-text">{{ pt('results') }}</router-link>
    </li>
  </ul>
</template>
