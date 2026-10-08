<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getAssessment, updateAssessment, deleteAssessment, type AssessmentDetail } from '@/api/platformExams'
import PageError from '@/components/platform/PageError.vue'
import { PlatformError } from '@/lib/platformApi'

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
// Students have started the exam: this page's Unpublish/Delete would be refused (409 has_attempts) for everyone, admins
// included — closing or archiving it is done from the admin exams screen. Also set when the server says so after the
// page was loaded (a student started in the meantime).
const started = ref(false)
const hasStarted = computed(() => started.value || (a.value?.attempt_count ?? 0) > 0)
function fail(e: unknown) {
  if (e instanceof PlatformError && e.code === 'has_attempts') started.value = true
  error.value = platformErrorMessage(pt, e)
}
async function unpublish() {
  try { await updateAssessment(id, { status: 'draft' }); await load() } catch (e) { fail(e) }
}
async function remove() {
  if (!window.confirm(pt('confirmDelete'))) return
  try { await deleteAssessment(id); router.replace('/platform') } catch (e) { fail(e) }
}
onMounted(load)
</script>

<template>
  <div v-if="a" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${a.subject_id}`" class="text-body-sm underline">{{ a.subject_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words" dir="auto">{{ a.title }}</h1>
    <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('by') }} {{ a.teacher_name }} · {{ a.question_count }} {{ pt('questionsCount') }} · {{ a.total_points }} {{ pt('points') }}
      <template v-if="a.duration_min"> · {{ a.duration_min }} {{ pt('minutesShort') }}</template>
      <template v-if="a.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <p v-if="a.description" class="text-body-lg mb-3 whitespace-pre-line" dir="auto">{{ a.description }}</p>
    <p v-if="a.opens_at || a.closes_at" class="text-body-sm mb-3">
      <template v-if="a.opens_at">{{ pt('opensAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.opens_at) }}</template>
      <template v-if="a.closes_at"> · {{ pt('closesAt').replace(/\s*\(.*\)/, '') }}: {{ fmt(a.closes_at) }}</template>
    </p>
    <p v-if="a.pass_mark || a.release_mode === 'after_close'" class="text-body-sm mb-3" data-testid="exam-terms">
      <template v-if="a.pass_mark">{{ pt('tkPassMark') }}: <span dir="ltr" class="inline-block">{{ a.pass_mark }}%</span></template><template v-if="a.pass_mark && a.release_mode === 'after_close'"> · </template><template v-if="a.release_mode === 'after_close'">{{ pt('tkResultAfterClose') }}</template>
    </p>
    <p v-if="a.status === 'closed' || a.status === 'archived'" class="text-body-md font-semibold mb-3" data-testid="exam-closed">{{ pt('tkClosedExam') }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <template v-if="auth.role === 'student'">
      <section v-if="a.can_start && !a.in_progress_attempt" class="card-filled p-4 mb-4 space-y-1" data-testid="exam-rules">
        <h2 class="font-bold">{{ pt('tkRulesTitle') }}</h2>
        <ul class="list-disc ps-5 text-body-sm space-y-0.5"><li>{{ pt('tkRules1') }}</li><li>{{ pt('tkRules2') }}</li><li v-if="a.shuffle_questions || a.shuffle_options">{{ pt('tkRules3') }}</li><li>{{ pt('tkRules4') }}</li></ul>
      </section>
      <p class="text-body-sm mb-3">{{ pt('attemptsUsed') }}: <span dir="ltr" class="inline-block">{{ a.attempts_used }} / {{ a.max_attempts }}</span></p>
      <router-link v-if="a.can_start" :to="`/platform/assessments/${a.id}/take`" class="btn-filled inline-flex mb-4">
        {{ a.in_progress_attempt ? pt('resumeAssessment') : pt('startAssessment') }}
      </router-link>
      <ul class="space-y-2">
        <li v-for="t in a.attempts.filter(x => x.status !== 'in_progress')" :key="t.attempt_id">
          <router-link :to="`/platform/attempts/${t.attempt_id}`" class="card-filled block p-3">
            <span v-if="t.status === 'submitted' && !t.released" class="text-body-sm" data-testid="attempt-embargo">{{ pt('tkResultAfterClose') }}</span>
            <span v-else-if="t.status === 'submitted'" class="font-bold inline-block" dir="ltr">{{ t.score }} / {{ t.total }}</span>
            <span v-else>{{ pt('expiredAttempt') }}</span>
            <span v-if="t.pending" class="text-body-sm ms-2">· {{ pt('pendingGrading') }}</span>
            <span v-if="t.passed !== null" class="text-body-sm ms-2 font-semibold">· {{ t.passed ? pt('tkPassed') : pt('tkFailed') }}</span>
            <span class="text-body-sm ms-2" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(t.started_at) }}</span>
          </router-link>
        </li>
      </ul>
    </template>

    <div v-if="!isOwner" class="mt-4"><ReportButton target-type="assessment" :target-id="a.id" /></div>

    <div v-if="isOwner || auth.role === 'admin' || (auth.role === 'teacher' && !a.teacher_id)" class="flex flex-wrap gap-2 mt-4">
      <router-link :to="`/platform/assessments/${a.id}/results`" class="btn-filled">{{ pt('results') }} ({{ a.attempt_count }})</router-link>
      <router-link v-if="isOwner" :to="`/platform/assessments/${a.id}/edit`" class="btn-outlined">{{ pt('edit') }}</router-link>
      <template v-if="auth.role === 'admin'">
        <template v-if="!hasStarted">
          <button v-if="a.status === 'published'" class="btn-outlined" data-testid="exam-unpublish" @click="unpublish">{{ pt('unpublish') }}</button>
          <button class="btn-outlined" data-testid="exam-delete" @click="remove">{{ pt('del') }}</button>
        </template>
        <router-link v-else to="/platform/admin/exams" class="btn-outlined" data-testid="exam-open-admin">{{ pt('openAdminExams') }}</router-link>
      </template>
    </div>
    <p v-if="auth.role === 'admin' && hasStarted" class="text-body-sm mt-2" data-testid="exam-admin-hint" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminExamsHint') }}</p>
  </div>
  <PageError v-else-if="error" :message="error" />
</template>
