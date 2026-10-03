<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getAssessment, updateAssessment, deleteAssessment, type AssessmentDetail } from '@/api/platformExams'

const pt = usePt()
const auth = useAuthStore()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string

const a = ref<AssessmentDetail | null>(null)
const error = ref('')
const isOwner = computed(() => a.value?.teacher_id === auth.profile?.id)
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load() {
  try {
    a.value = await getAssessment(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function unpublish() {
  try { await updateAssessment(id, { status: 'draft' }); await load() } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function remove() {
  if (!window.confirm(pt('confirmDelete'))) return
  try { await deleteAssessment(id); router.replace('/platform') } catch (e) { error.value = platformErrorMessage(pt, e) }
}
onMounted(load)
</script>

<template>
  <div v-if="a" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${a.subject_id}`" class="text-body-sm underline">{{ a.subject_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words">{{ a.title }}</h1>
    <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('by') }} {{ a.teacher_name }} · {{ a.question_count }} {{ pt('questionsCount') }} · {{ a.total_points }} {{ pt('points') }}
      <template v-if="a.duration_min"> · {{ a.duration_min }} {{ pt('minutesShort') }}</template>
      <template v-if="a.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <p v-if="a.description" class="text-body-lg mb-3 whitespace-pre-line">{{ a.description }}</p>
    <p v-if="a.opens_at || a.closes_at" class="text-body-sm mb-3">
      <template v-if="a.opens_at">{{ pt('opensAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.opens_at) }}</template>
      <template v-if="a.closes_at"> · {{ pt('closesAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.closes_at) }}</template>
    </p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <template v-if="auth.role === 'student'">
      <p class="text-body-sm mb-3">{{ pt('attemptsUsed') }}: <span dir="ltr" class="inline-block">{{ a.attempts_used }} / {{ a.max_attempts }}</span></p>
      <router-link v-if="a.can_start" :to="`/platform/assessments/${a.id}/take`" class="btn-filled inline-flex mb-4">
        {{ a.in_progress_attempt ? pt('resumeAssessment') : pt('startAssessment') }}
      </router-link>
      <ul class="space-y-2">
        <li v-for="t in a.attempts.filter(x => x.status !== 'in_progress')" :key="t.attempt_id">
          <router-link :to="`/platform/attempts/${t.attempt_id}`" class="card-filled block p-3">
            <span v-if="t.status === 'submitted'" class="font-bold inline-block" dir="ltr">{{ t.score }} / {{ t.total }}</span>
            <span v-else>{{ pt('expiredAttempt') }}</span>
            <span v-if="t.pending" class="text-body-sm ms-2">· {{ pt('pendingGrading') }}</span>
            <span class="text-body-sm ms-2" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(t.started_at) }}</span>
          </router-link>
        </li>
      </ul>
    </template>

    <div v-if="!isOwner" class="mt-4"><ReportButton target-type="assessment" :target-id="a.id" /></div>

    <div v-if="isOwner || auth.role === 'admin'" class="flex flex-wrap gap-2 mt-4">
      <router-link :to="`/platform/assessments/${a.id}/results`" class="btn-filled">{{ pt('results') }} ({{ a.attempt_count }})</router-link>
      <router-link v-if="isOwner" :to="`/platform/assessments/${a.id}/edit`" class="btn-outlined">{{ pt('edit') }}</router-link>
      <template v-if="auth.role === 'admin'">
        <button v-if="a.status === 'published'" class="btn-outlined" @click="unpublish">{{ pt('unpublish') }}</button>
        <button class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </template>
    </div>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
