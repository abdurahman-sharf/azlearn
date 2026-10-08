<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import {
  adminTeaching, adminTeachingImpact, decideTeaching,
  type AdminTeachingRow, type TeachingImpact, type TeachingStatus,
} from '@/api/platformLearning'
import { useAdminStats } from '@/lib/adminStats'
import { impactTotal, placeLabel } from '@/utils/teachingCards'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

// The admin's queue of teaching requests: waiting ones first (oldest first), the approved ones (an approval can be
// withdrawn - the dialog says how many items of the teacher stop being visible), and the rejected ones. Every decision can
// carry a short note that the teacher sees.
const pt = usePt()
const i18n = useI18nStore()
const { refresh: refreshAdminStats } = useAdminStats()

const PAGE = 25
const TABS: TeachingStatus[] = ['pending', 'approved', 'rejected']
const TAB_LABEL: Record<TeachingStatus, PlatformKey> = { pending: 'statusPending', approved: 'statusApproved', rejected: 'statusRejected' }
const EMPTY: Record<TeachingStatus, PlatformKey> = { pending: 'noRequests', approved: 'atEmptyApproved', rejected: 'atEmptyRejected' }
const accountKey: Record<string, PlatformKey> = { active: 'statusActive', pending: 'statusPending', rejected: 'statusRejected', suspended: 'statusSuspended' }

const tab = ref<TeachingStatus>('pending')
const search = ref('')
const query = ref('') // the search that is applied (the box applies on submit)
const page = ref(0)
const items = ref<AdminTeachingRow[]>([])
const counts = reactive<Record<TeachingStatus, number>>({ pending: 0, approved: 0, rejected: 0 })
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const notice = ref('')
const notes = reactive<Record<string, string>>({})
const working = ref('')
let seq = 0

const key = (t: Pick<AdminTeachingRow, 'teacher_id' | 'subject_id'>) => `${t.teacher_id}:${t.subject_id}`
const pages = computed(() => Math.max(1, Math.ceil(counts[tab.value] / PAGE)))
const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load() {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const q = query.value || undefined
    // the open tab fetches its page; the other two only need their totals for the tab labels
    const res = await Promise.all(TABS.map((s) => adminTeaching(s === tab.value ? { status: s, q, limit: PAGE, offset: page.value * PAGE } : { status: s, q, limit: 1 })))
    if (mine !== seq) return // a newer request is on its way
    TABS.forEach((s, i) => { counts[s] = res[i]!.total })
    const current = res[TABS.indexOf(tab.value)]!
    if (!current.items.length && page.value > 0 && current.total > 0) {
      page.value = Math.ceil(current.total / PAGE) - 1 // the last row of the last page was just decided
      loading.value = false
      return load()
    }
    items.value = current.items
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === seq) {
      loading.value = false
      loaded.value = true
    }
  }
}

function pick(s: TeachingStatus) {
  if (tab.value === s) return
  tab.value = s
  page.value = 0
  notice.value = ''
  load()
}
function submitSearch() {
  query.value = search.value.trim()
  page.value = 0
  load()
}
function go(delta: number) {
  page.value += delta
  load()
}

async function decide(t: AdminTeachingRow, status: TeachingStatus) {
  if (working.value) return
  working.value = key(t)
  error.value = ''
  notice.value = ''
  try {
    await decideTeaching({ teacher_id: t.teacher_id, subject_id: t.subject_id, status, reason: notes[key(t)]?.trim() || undefined })
    delete notes[key(t)]
    notice.value = pt('bankDone')
    await load()
    refreshAdminStats() // the sidebar badge counts the waiting requests
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    working.value = ''
  }
}

// --- withdrawing an approval ---------------------------------------------------------------------------------------------
const revoking = ref<AdminTeachingRow | null>(null)
const impact = ref<TeachingImpact | null>(null)
const impactFailed = ref(false)
const loadingImpact = ref('')
const revokeBusy = ref(false)
const revokeError = ref('')

async function askRevoke(t: AdminTeachingRow) {
  if (loadingImpact.value) return
  error.value = ''
  notice.value = ''
  revokeError.value = ''
  impact.value = null
  impactFailed.value = false
  loadingImpact.value = key(t)
  try {
    impact.value = await adminTeachingImpact(t.teacher_id, t.subject_id)
  } catch {
    impactFailed.value = true // the decision stays possible; the dialog just cannot give the numbers
  } finally {
    loadingImpact.value = ''
  }
  revoking.value = t
}

async function confirmRevoke() {
  const t = revoking.value
  if (!t || revokeBusy.value) return
  revokeBusy.value = true
  revokeError.value = ''
  try {
    await decideTeaching({ teacher_id: t.teacher_id, subject_id: t.subject_id, status: 'rejected', reason: notes[key(t)]?.trim() || undefined })
    delete notes[key(t)]
    revoking.value = null
    notice.value = pt('bankDone')
    await load()
    refreshAdminStats()
  } catch (e) {
    revokeError.value = platformErrorMessage(pt, e)
  } finally {
    revokeBusy.value = false
  }
}

const revokeMessage = computed(() => {
  const t = revoking.value
  if (!t) return ''
  const base = pt('atRevokeMsg').replace('{teacher}', t.teacher_name)
  if (impactFailed.value) return `${base} ${pt('atImpactUnknown')}`
  return `${base} ${impactTotal(impact.value) > 0 ? pt('atImpactSome') : pt('atImpactNone')}`
})
const revokeCounts = computed(() => {
  const i = impact.value
  if (!i) return []
  return [
    { label: pt('tchImpactPosts'), value: i.posts },
    { label: pt('tchImpactCourses'), value: i.courses },
    { label: pt('tchImpactLive'), value: i.live },
    { label: pt('tchImpactExams'), value: i.exams },
  ]
})

onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="admin-teaching-page">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminTeaching') }}</h1>

    <form class="mb-3 flex gap-2" role="search" :aria-label="pt('adminTeaching')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" :placeholder="pt('atSearchPlaceholder')" :aria-label="pt('atSearchPlaceholder')" class="input-outlined flex-1 min-w-0" maxlength="100" data-testid="admin-teaching-search" />
      <button type="submit" class="btn-tonal" data-testid="admin-teaching-search-go">{{ pt('atSearchGo') }}</button>
    </form>

    <div class="flex flex-wrap gap-2 mb-1" role="group" :aria-label="pt('adminTeaching')">
      <button
        v-for="s in TABS" :key="s" type="button" :class="tab === s ? 'btn-filled' : 'btn-outlined'" :aria-pressed="tab === s"
        :data-testid="`admin-teaching-tab-${s}`" @click="pick(s)"
      >{{ pt(TAB_LABEL[s]) }} <span dir="ltr" class="inline-block" :data-testid="`admin-teaching-count-${s}`">({{ counts[s] }})</span></button>
    </div>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ pt('atOldestFirst') }}</p>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="admin-teaching-error">{{ error }}</p>
    <p v-if="notice" class="text-body-sm mb-3" role="status" data-testid="admin-teaching-notice">{{ notice }}</p>
    <p v-if="loaded && !loading && !items.length && !error" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="admin-teaching-empty">{{ query ? pt('noResults') : pt(EMPTY[tab]) }}</p>

    <ul class="space-y-3" data-testid="admin-teaching-list">
      <li v-for="t in items" :key="key(t)" class="card-filled p-4 space-y-3" :data-testid="`admin-teaching-row-${t.subject_id}`" :data-teacher="t.teacher_id">
        <div class="flex items-start gap-3">
          <div class="flex-1 min-w-0 space-y-1">
            <div>
              <span class="text-body-sm">{{ pt('teacher') }}:</span> <router-link :to="`/platform/teachers/${t.teacher_id}`" class="font-bold underline break-words" dir="auto">{{ t.teacher_name }}</router-link>
              <!-- a request can come from an account that is still waiting for its own approval; approving the subject does not activate it -->
              <span v-if="t.teacher_status && t.teacher_status !== 'active'" class="ms-2 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" data-testid="teacher-account-status">{{ pt('teacherAccountStatus') }}: {{ pt(accountKey[t.teacher_status] ?? 'statusPending') }}</span>
            </div>
            <div>
              <span class="text-body-sm">{{ pt('subject') }}:</span> <span class="font-bold break-words" dir="auto">{{ t.subject_name }}</span>
              <span v-if="t.subject_active === false" class="ms-2 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))">{{ pt('tchSubjectOff') }}</span>
              <span v-if="t.institution_active === false" class="ms-2 px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))">{{ pt('tchInstitutionOff') }}</span>
            </div>
            <div class="text-body-sm break-words" style="color: rgb(var(--md-on-surface-variant))" dir="auto" :data-testid="`admin-teaching-place-${t.subject_id}`">{{ placeLabel(t.institution_name, t.unit_path) }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <span v-if="t.created_at">{{ pt('tchRequestedAt') }}: {{ fmt(t.created_at) }}</span>
              <span v-if="t.decided_at"> · {{ pt('tchDecidedAt') }}: {{ fmt(t.decided_at) }}</span>
            </div>
            <p v-if="t.reason" class="text-body-sm"><b>{{ pt('tchAdminNote') }}:</b> <span dir="auto" class="break-words">{{ t.reason }}</span></p>
          </div>
        </div>

        <label class="block">
          <span class="text-label-lg">{{ pt('atNote') }}</span>
          <input v-model="notes[key(t)]" maxlength="300" dir="auto" class="input-outlined w-full mt-1" :aria-label="`${pt('atNote')}: ${t.teacher_name} — ${t.subject_name}`" :data-testid="`admin-teaching-note-${t.subject_id}`" />
        </label>

        <div class="flex flex-wrap gap-2">
          <button v-if="t.status !== 'approved'" type="button" class="btn-filled" :disabled="!!working" :aria-label="`${pt('approve')}: ${t.teacher_name} — ${t.subject_name}`" :data-testid="`admin-teaching-approve-${t.subject_id}`" @click="decide(t, 'approved')">{{ pt('approve') }}</button>
          <button v-if="t.status === 'pending'" type="button" class="btn-outlined" :disabled="!!working" :aria-label="`${pt('reject')}: ${t.teacher_name} — ${t.subject_name}`" :data-testid="`admin-teaching-reject-${t.subject_id}`" @click="decide(t, 'rejected')">{{ pt('reject') }}</button>
          <button v-if="t.status === 'approved'" type="button" class="btn-outlined" :disabled="!!loadingImpact || !!working" :aria-label="`${pt('atRevoke')}: ${t.teacher_name} — ${t.subject_name}`" :data-testid="`admin-teaching-revoke-${t.subject_id}`" @click="askRevoke(t)">{{ pt('atRevoke') }}</button>
        </div>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3 mt-4" :aria-label="pt('pageLabel')" data-testid="admin-teaching-pager">
      <button type="button" class="btn-outlined" :disabled="page === 0 || loading" data-testid="admin-teaching-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ page + 1 }} / {{ pages }}</span></span>
      <button type="button" class="btn-outlined" :disabled="page + 1 >= pages || loading" data-testid="admin-teaching-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>

    <ConfirmDeleteDialog
      v-if="revoking"
      :title="`${pt('atRevokeTitle')}: ${revoking.subject_name}`"
      :message="revokeMessage"
      :counts="revokeCounts"
      :kept="pt('tchKeptNothingDeleted')"
      :confirm-label="pt('atRevoke')"
      :busy="revokeBusy"
      :error="revokeError"
      @confirm="confirmRevoke"
      @close="revoking = null"
    />
  </div>
</template>
