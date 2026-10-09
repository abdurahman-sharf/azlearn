<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, platformAdminErrorMessage } from '@/i18n/platform'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getAssessment, type AssessmentDetail } from '@/api/platformExams'
import { adminExams, type ExamAction } from '@/api/platformExamAdmin'
import PageError from '@/components/platform/PageError.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
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
// Students have started the exam: deleting it would erase their results, so the admin's delete asks for the exam's title
// (the server demands it too) and closing or archiving is the better choice. Also set when the server says so after the
// page was loaded (a student started in the meantime).
const started = ref(false)
const hasStarted = computed(() => started.value || (a.value?.attempt_count ?? 0) > 0)
const isAdmin = computed(() => auth.role === 'admin')
/** an exam the admin made himself (a teacher's is only moderated: closed, archived, reopened, deleted) */
const adminOwned = computed(() => !a.value?.teacher_id)
const editLink = computed(() => (isOwner.value ? `/platform/exams/${id}/edit` : isAdmin.value && adminOwned.value ? `/platform/admin/exams/${id}/edit` : ''))
const busy = ref(false)
/** The server says attempts exist (`has_attempts`, or `confirm_required` = the typed title is needed): the page was loaded before the first student started. */
const startedMeanwhile = (e: unknown) => e instanceof PlatformError && (e.code === 'has_attempts' || e.code === 'confirm_required')
function fail(e: unknown) {
  if (startedMeanwhile(e)) started.value = true
  error.value = platformAdminErrorMessage(pt, e)
}
/** The admin's lifecycle actions: the same server rules as the admin exams screen. */
async function act(action: ExamAction) {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    await adminExams.action(id, action)
    await load()
  } catch (e) {
    fail(e)
    await load()
  } finally {
    busy.value = false
  }
}
const removing = ref(false)
const removeError = ref('')
function askRemove() {
  removeError.value = ''
  removing.value = true
}
async function remove() {
  if (!a.value || busy.value) return
  busy.value = true
  removeError.value = ''
  try {
    await adminExams.remove(id, hasStarted.value ? a.value.title : undefined)
    removing.value = false
    await router.replace('/platform/admin/exams')
  } catch (e) {
    removeError.value = platformAdminErrorMessage(pt, e)
    if (startedMeanwhile(e)) {
      // a student started after this page loaded: the dialog now asks for the exam's title, with the real attempt count
      started.value = true
      await load()
    }
  } finally {
    busy.value = false
  }
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

    <div v-if="isOwner || isAdmin || (auth.role === 'teacher' && !a.teacher_id)" class="flex flex-wrap gap-2 mt-4">
      <router-link :to="`/platform/assessments/${a.id}/results`" class="btn-filled">{{ pt('results') }} ({{ a.attempt_count }})</router-link>
      <router-link v-if="editLink" :to="editLink" class="btn-outlined" data-testid="exam-edit">{{ pt('edit') }}</router-link>
      <router-link v-if="isOwner" to="/platform/exams" class="btn-text" data-testid="exam-open-mine">{{ pt('mexTitle') }}</router-link>
      <template v-if="isAdmin">
        <button v-if="a.status === 'published' && !hasStarted" class="btn-outlined" :disabled="busy" data-testid="exam-unpublish" @click="act('unpublish')">{{ pt('unpublish') }}</button>
        <button v-if="a.status === 'published'" class="btn-outlined" :disabled="busy" data-testid="exam-close" @click="act('close')">{{ pt('exClose') }}</button>
        <button v-if="a.status === 'closed'" class="btn-outlined" :disabled="busy" data-testid="exam-reopen" @click="act('reopen')">{{ pt('exReopen') }}</button>
        <button v-if="a.status === 'draft' || a.status === 'closed'" class="btn-outlined" :disabled="busy" data-testid="exam-archive" @click="act('archive')">{{ pt('exArchive') }}</button>
        <button v-if="a.status === 'archived'" class="btn-outlined" :disabled="busy" data-testid="exam-restore" @click="act('restore')">{{ pt('exRestore') }}</button>
        <button v-if="a.locked" class="btn-outlined" :disabled="busy" data-testid="exam-unlock" @click="act('unlock')">{{ pt('exUnlock') }}</button>
        <button class="btn-outlined" :disabled="busy" data-testid="exam-delete" @click="askRemove">{{ pt('del') }}</button>
        <router-link to="/platform/admin/exams" class="btn-text" data-testid="exam-open-admin">{{ pt('openAdminExams') }}</router-link>
      </template>
    </div>
    <p v-if="isAdmin && hasStarted" class="text-body-sm mt-2" data-testid="exam-admin-hint" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminExamsHint') }}</p>
    <ConfirmDeleteDialog
      v-if="removing"
      :title="`${pt('del')}: ${a.title}`"
      :message="hasStarted ? pt('exDeleteMsgAttempts') : pt('exDeleteMsg')"
      :counts="[{ label: pt('exAttemptsCount'), value: a.attempt_count }, { label: pt('questionsCount'), value: a.question_count }]"
      :confirm-name="hasStarted ? a.title : undefined"
      :busy="busy"
      :error="removeError"
      @confirm="remove"
      @close="removing = false"
    />
  </div>
  <PageError v-else-if="error" :message="error" />
</template>
