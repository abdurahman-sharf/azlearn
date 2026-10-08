<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myTeaching, requestTeaching, dropTeaching, teachingImpact, type MyTeaching, type TeachingImpact } from '@/api/platformLearning'
import { useTeacherStats } from '@/lib/teacherStats'
import { groupByStatus, impactTotal, isUsable, placeLabel, statusBySubject, type TeachingGroupKey } from '@/utils/teachingCards'
import SubjectPicker from '@/components/platform/SubjectPicker.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

// "My subjects": where a teacher asks for subjects (one request after another), follows each request (dates, the admin's
// note, where the subject sits) and starts new content for an approved subject. It works for an active teacher and also
// for one whose account is still waiting for approval (the server lets a pending teacher browse the catalogue and manage
// requests, nothing else). Withdrawing deletes nothing: the dialog says how many items stop being visible.
const pt = usePt()
const auth = useAuthStore()
const i18n = useI18nStore()
const { stats, refresh: refreshStats } = useTeacherStats()

const items = ref<MyTeaching[]>([])
const loaded = ref(false)
const loadFailed = ref(false)
const error = ref('')
const sent = ref(false)
const busy = ref(false)
const subjectId = ref('')
const retrying = ref('')

const GROUP_TITLE: Record<TeachingGroupKey, PlatformKey> = { approved: 'tchGrpApproved', pending: 'tchGrpPending', rejected: 'tchGrpRejected' }
const STATUS_KEY: Record<TeachingGroupKey, PlatformKey> = { pending: 'statusPending', approved: 'statusApproved', rejected: 'statusRejected' }

const accountPending = computed(() => auth.profile?.status === 'pending')
const groups = computed(() => groupByStatus(items.value))
const statusMap = computed(() => statusBySubject(items.value))
// The admin can switch exam creation off; the exam shortcut then says why instead of leading to an error.
const examsOff = computed(() => stats.value?.can_create_exams === false)
const approvedCount = computed(() => items.value.filter((t) => t.status === 'approved').length)

/** The subject page opens for an active account and a subject that is switched on, in an institution that is switched on. */
const canOpenSubject = (t: MyTeaching) => auth.isActive && t.subject_active !== false && t.institution_active !== false

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load() {
  try {
    items.value = await myTeaching()
    loadFailed.value = false
  } catch (e) {
    loadFailed.value = true
    error.value = platformErrorMessage(pt, e)
  } finally {
    loaded.value = true
  }
}

async function request() {
  if (!subjectId.value || busy.value) return
  busy.value = true
  error.value = ''
  sent.value = false
  try {
    await requestTeaching(subjectId.value)
    sent.value = true
    subjectId.value = '' // the picker keeps its place: the next subject of the same level is one click away
    await load()
    if (auth.isActive) refreshStats()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}

async function retry(t: MyTeaching) {
  if (retrying.value) return
  retrying.value = t.subject_id
  error.value = ''
  sent.value = false
  try {
    await requestTeaching(t.subject_id)
    sent.value = true
    await load()
    if (auth.isActive) refreshStats()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    retrying.value = ''
  }
}

// --- withdrawing -------------------------------------------------------------------------------------------------------
const dropping = ref<MyTeaching | null>(null)
const impact = ref<TeachingImpact | null>(null)
const impactFailed = ref(false)
const loadingImpact = ref('')
const dropBusy = ref(false)
const dropError = ref('')

/** Opens the dialog; for an approved subject it first asks the server what would stop being visible. */
async function askDrop(t: MyTeaching) {
  if (loadingImpact.value) return
  error.value = ''
  dropError.value = ''
  impact.value = null
  impactFailed.value = false
  if (t.status === 'approved') {
    loadingImpact.value = t.subject_id
    try {
      impact.value = await teachingImpact(t.subject_id)
    } catch {
      impactFailed.value = true // the withdrawal is still possible; the dialog just cannot give the numbers
    } finally {
      loadingImpact.value = ''
    }
  }
  dropping.value = t
}

async function confirmDrop() {
  const t = dropping.value
  if (!t || dropBusy.value) return
  dropBusy.value = true
  dropError.value = ''
  try {
    await dropTeaching(t.subject_id)
    dropping.value = null
    sent.value = false
    await load()
    if (auth.isActive) refreshStats()
  } catch (e) {
    dropError.value = platformErrorMessage(pt, e)
  } finally {
    dropBusy.value = false
  }
}

const dropMessage = computed(() => {
  const t = dropping.value
  if (!t) return ''
  if (t.status === 'pending') return pt('tchDropPendingMsg')
  if (t.status === 'rejected') return pt('tchDropRejectedMsg')
  if (impactFailed.value) return pt('tchDropUnknownMsg')
  return impactTotal(impact.value) > 0 ? pt('tchDropApprovedMsg') : pt('tchDropNothingMsg')
})
const dropCounts = computed(() => {
  const i = impact.value
  if (!i) return []
  return [
    { label: pt('tchImpactPosts'), value: i.posts },
    { label: pt('tchImpactCourses'), value: i.courses },
    { label: pt('tchImpactLive'), value: i.live },
    { label: pt('tchImpactExams'), value: i.exams },
  ]
})

onMounted(() => {
  load()
  if (auth.isActive) refreshStats()
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="teaching-page">
    <router-link :to="auth.isActive ? '/platform/teacher' : '/auth/pending'" class="text-body-sm underline" data-testid="teaching-back">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myTeaching') }}</h1>

    <p v-if="accountPending" class="card-filled p-3 text-body-sm mb-4" role="note" data-testid="teaching-account-pending">{{ pt('tchReqPendingAccount') }}</p>

    <section class="card-filled p-4 mb-6 space-y-3" aria-labelledby="teaching-request-title" data-testid="teaching-request-card">
      <h2 id="teaching-request-title" class="text-title-md font-bold">{{ pt('tchReqTitle') }}</h2>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('addTeachingHint') }}</p>
      <!-- subjects already asked for (waiting or approved) are listed but cannot be picked; a rejected one can be asked again -->
      <SubjectPicker v-model="subjectId" :remember="false" active-only :status-by-subject="statusMap" />
      <p v-if="sent" class="text-body-sm" role="status" data-testid="teaching-sent">{{ pt('tchReqSent') }}</p>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="teaching-error">{{ error }}</p>
      <button type="button" class="btn-filled" :disabled="!subjectId || busy" data-testid="teaching-request" @click="request">{{ pt('requestTeaching') }}</button>
    </section>

    <p v-if="loaded && !loadFailed && !items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="teaching-empty">{{ pt('tchEmpty') }}</p>
    <p v-if="examsOff && approvedCount" id="teaching-exams-off" class="text-body-sm mb-3" style="color: rgb(var(--md-on-surface-variant))" data-testid="teaching-exams-off">{{ pt('examsOffReason') }}</p>

    <section v-for="g in groups" :key="g.status" class="mb-6" :aria-labelledby="`teaching-group-${g.status}`" :data-testid="`teaching-group-${g.status}`">
      <h2 :id="`teaching-group-${g.status}`" class="text-title-md font-bold mb-3">{{ pt(GROUP_TITLE[g.status]) }} <span dir="ltr" class="inline-block text-body-sm font-normal">({{ g.rows.length }})</span></h2>
      <ul class="space-y-3" :data-testid="g.status === 'approved' ? 'teaching-list' : `teaching-list-${g.status}`">
        <li v-for="t in g.rows" :key="t.subject_id" class="card-filled p-4 space-y-3" :data-testid="`teaching-card-${t.subject_id}`">
          <div class="flex flex-wrap items-start gap-2" :data-testid="`teaching-item-${t.subject_id}`">
            <div class="flex-1 min-w-[10rem]">
              <!-- plain text where the page would not open: a pending teacher cannot open subject pages yet, and a subject or
                   institution that is switched off answers "not found" to everyone but an admin -->
              <router-link v-if="canOpenSubject(t)" :to="`/platform/subjects/${t.subject_id}`" class="font-bold break-words underline" dir="auto">{{ t.subject_name }}</router-link>
              <div v-else class="font-bold break-words" dir="auto">{{ t.subject_name }}</div>
              <div class="text-body-sm break-words" style="color: rgb(var(--md-on-surface-variant))" dir="auto" :data-testid="`teaching-place-${t.subject_id}`">{{ placeLabel(t.institution_name, t.unit_path) }}</div>
            </div>
            <div class="flex flex-wrap justify-end gap-1">
              <span class="px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-surface-container-high))" :data-testid="`teaching-status-${t.subject_id}`">{{ pt(STATUS_KEY[t.status]) }}</span>
              <span v-if="t.subject_active === false" class="px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" :data-testid="`teaching-subject-off-${t.subject_id}`">{{ pt('tchSubjectOff') }}</span>
              <span v-if="t.institution_active === false" class="px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" :data-testid="`teaching-institution-off-${t.subject_id}`">{{ pt('tchInstitutionOff') }}</span>
            </div>
          </div>

          <dl class="text-body-sm grid grid-cols-1 sm:grid-cols-2 gap-x-4 gap-y-1" style="color: rgb(var(--md-on-surface-variant))">
            <div v-if="t.created_at"><dt class="inline font-semibold">{{ pt('tchRequestedAt') }}:</dt> <dd class="inline">{{ fmt(t.created_at) }}</dd></div>
            <div v-if="t.decided_at"><dt class="inline font-semibold">{{ pt('tchDecidedAt') }}:</dt> <dd class="inline">{{ fmt(t.decided_at) }}</dd></div>
          </dl>
          <p v-if="t.reason" class="text-body-sm rounded-xl px-3 py-2" style="background-color: rgb(var(--md-surface-container-high))" :data-testid="`teaching-reason-${t.subject_id}`">
            <b>{{ pt('tchAdminNote') }}:</b> <span dir="auto" class="break-words">{{ t.reason }}</span>
          </p>
          <p v-if="t.status === 'pending'" class="text-body-sm" role="note">{{ pt('tchPendingHint') }}</p>
          <p v-else-if="t.status === 'rejected'" class="text-body-sm" role="note">{{ pt('tchRejectedHint') }}</p>
          <p v-else-if="!isUsable(t)" class="text-body-sm" role="note" :data-testid="`teaching-unusable-${t.subject_id}`">{{ pt('tchUnusableHint') }}</p>

          <p v-if="t.status === 'approved' && isUsable(t) && !auth.isActive" class="text-body-sm" role="note" :data-testid="`teaching-account-pending-${t.subject_id}`">{{ pt('tchNoCreatePending') }}</p>

          <div class="flex flex-wrap gap-2 items-center">
            <!-- the editors need an active account; for a pending one they would only bounce to the waiting page -->
            <template v-if="t.status === 'approved' && isUsable(t) && auth.isActive">
              <router-link :to="`/platform/posts/new?subject=${t.subject_id}`" class="btn-tonal" :aria-label="`${pt('newPost')}: ${t.subject_name}`" :data-testid="`teaching-new-post-${t.subject_id}`">{{ pt('newPost') }}</router-link>
              <router-link :to="`/platform/courses/new?subject=${t.subject_id}`" class="btn-tonal" :aria-label="`${pt('newCourse')}: ${t.subject_name}`" :data-testid="`teaching-new-course-${t.subject_id}`">{{ pt('newCourse') }}</router-link>
              <router-link :to="`/platform/live/new?subject=${t.subject_id}`" class="btn-tonal" :aria-label="`${pt('newLive')}: ${t.subject_name}`" :data-testid="`teaching-new-live-${t.subject_id}`">{{ pt('newLive') }}</router-link>
              <router-link v-if="!examsOff" :to="`/platform/assessments/new?subject=${t.subject_id}`" class="btn-tonal" :aria-label="`${pt('tchNewExam')}: ${t.subject_name}`" :data-testid="`teaching-new-exam-${t.subject_id}`">{{ pt('tchNewExam') }}</router-link>
              <button v-else type="button" class="btn-tonal" disabled aria-describedby="teaching-exams-off" :aria-label="`${pt('tchNewExam')}: ${t.subject_name}`" :data-testid="`teaching-new-exam-${t.subject_id}`">{{ pt('tchNewExam') }}</button>
            </template>
            <button v-if="t.status === 'rejected'" type="button" class="btn-filled" :disabled="!!retrying" :aria-label="`${pt('tchRetry')}: ${t.subject_name}`" :data-testid="`teaching-retry-${t.subject_id}`" @click="retry(t)">{{ pt('tchRetry') }}</button>
            <button type="button" class="btn-text ms-auto" :disabled="!!loadingImpact" :aria-label="`${t.status === 'rejected' ? pt('tchRemove') : pt('tchWithdraw')}: ${t.subject_name}`" :data-testid="`teaching-drop-${t.subject_id}`" @click="askDrop(t)">{{ t.status === 'rejected' ? pt('tchRemove') : pt('tchWithdraw') }}</button>
          </div>
        </li>
      </ul>
    </section>

    <ConfirmDeleteDialog
      v-if="dropping"
      :title="`${dropping.status === 'approved' ? pt('tchDropTitle') : dropping.status === 'pending' ? pt('tchDropRequestTitle') : pt('tchRemove')}: ${dropping.subject_name}`"
      :message="dropMessage"
      :counts="dropCounts"
      :kept="dropping.status === 'approved' ? pt('tchKeptNothingDeleted') : undefined"
      :confirm-label="dropping.status === 'rejected' ? pt('tchRemove') : pt('tchWithdraw')"
      :busy="dropBusy"
      :error="dropError"
      @confirm="confirmDrop"
      @close="dropping = null"
    />
  </div>
</template>
