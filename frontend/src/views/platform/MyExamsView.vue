<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { useTeacherStats } from '@/lib/teacherStats'
import { teacherExams, type ExamAction, type ExamRow } from '@/api/platformExamAdmin'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { isHiddenFromStudents } from '@/utils/contentHub'
import { usableSubjects } from '@/utils/teachingCards'
import { MY_PER, MY_TABS, clampPage, filtersFromQuery, filtersToQuery, listFilter, rowActions, type MyFilters, type MyTab, type RowAction } from '@/utils/myExams'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import HiddenWhy from '@/components/platform/HiddenWhy.vue'

// "My exams": the teacher's own exams with their state, counts and lifecycle actions (publish, unpublish, close, reopen,
// archive, restore, copy, delete), plus a read-only tab with the admin's exams for the subjects the teacher is approved for
// (results and grading only). The filters live in the address (back/forward and reloads keep them). An exam that students
// have taken cannot be deleted: the delete button is replaced by an explanation and close/archive stay.
const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()

const filters = reactive<MyFilters>(filtersFromQuery(route.query))
const search = ref(filters.q)
const items = ref<ExamRow[]>([])
const total = ref(0)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const notice = ref('')
const acting = ref('')
const teaching = ref<MyTeaching[]>([])
const teachingKnown = ref(false)
let seq = 0

const deleting = ref<ExamRow | null>(null)
const deleteBusy = ref(false)
const deleteError = ref('')

// The admin can switch exam creation off: new / publish / copy are then disabled with the reason; list, close, reopen,
// archive and results stay usable.
const { stats, refresh } = useTeacherStats()
const examsOff = computed(() => stats.value?.can_create_exams === false)

// Creating needs an approved subject that is switched on. While the list is unknown the button stays available (the editor
// explains itself); once known with none, the page points at "My subjects".
const hasUsableSubject = computed(() => !teachingKnown.value || usableSubjects(teaching.value).length > 0)
const subjectOptions = computed(() => {
  const seen = new Map<string, string>()
  for (const t of teaching.value) if (t.status === 'approved' && !seen.has(t.subject_id)) seen.set(t.subject_id, t.subject_name)
  return [...seen].map(([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name))
})

const TAB_LABEL: Record<MyTab, PlatformKey> = { all: 'exAll', draft: 'exPhase_draft', published: 'exPhase_published', closed: 'exPhase_closed', archived: 'exPhase_archived', admin: 'mexTabAdmin' }
const pages = computed(() => Math.max(1, Math.ceil(total.value / MY_PER)))
const filtered = computed(() => !!filters.q.trim() || filters.tab !== 'all' || !!filters.subjectId)
const adminTab = computed(() => filters.tab === 'admin')

/** `keepError`: the reload after a refused action - the message explaining the refusal must survive it. */
async function load(opts: { keepError?: boolean } = {}): Promise<void> {
  const mine = ++seq
  loading.value = true
  if (!opts.keepError) error.value = ''
  try {
    const r = await teacherExams.list(listFilter(filters, MY_PER))
    if (mine !== seq) return
    // the last row of the last page was just removed or moved to another tab: step back to a page that exists
    const page = clampPage(filters.page, r.total)
    if (page !== filters.page && !r.items.length) {
      filters.page = page
      syncQuery()
      loading.value = false
      return load(opts)
    }
    items.value = r.items
    total.value = r.total
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === seq) {
      loading.value = false
      loaded.value = true
    }
  }
}

function syncQuery() {
  router.replace({ query: filtersToQuery(filters) }).catch(() => { /* a redundant navigation is not an error */ })
}
/** A filter changed: back to the first page, keep the address in step, fetch. */
function refilter() {
  filters.page = 0
  syncQuery()
  load()
}

// The address changed from outside (a sidebar link back to "My exams", the browser's back button): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/exams') return // already leaving for another page
  const next = filtersFromQuery(route.query)
  if (JSON.stringify(filtersToQuery(next)) === JSON.stringify(filtersToQuery(filters))) return // our own update
  Object.assign(filters, next)
  search.value = next.q
  load()
})

let timer: ReturnType<typeof setTimeout> | undefined
watch(search, (v) => {
  clearTimeout(timer)
  timer = setTimeout(() => {
    if (v.trim() !== filters.q.trim()) {
      filters.q = v.trim()
      refilter()
    }
  }, 350)
})
onBeforeUnmount(() => clearTimeout(timer))

function submitSearch() {
  clearTimeout(timer)
  filters.q = search.value.trim()
  refilter()
}
function setTab(t: MyTab) {
  if (filters.tab === t) return
  filters.tab = t
  refilter()
}
function clearFilters() {
  search.value = ''
  Object.assign(filters, { tab: 'all', q: '', subjectId: '', page: 0 })
  syncQuery()
  load()
}
function go(delta: number) {
  filters.page = Math.max(0, filters.page + delta)
  syncQuery()
  load()
}

// ── rows
const fmt = (ms: number | null) => (ms ? new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '')
// Decided by the row itself, not by the current tab: for a moment after a tab switch the list still holds the previous tab's rows.
const readOnlyRow = (e: ExamRow) => e.owned === false
const acts = (e: ExamRow) => rowActions(e, { examsOff: examsOff.value, readOnly: readOnlyRow(e) })
const titleLink = (e: ExamRow) => (!readOnlyRow(e) && e.can_edit ? `/platform/exams/${e.id}/edit` : `/platform/assessments/${e.id}`)
const isHidden = (e: ExamRow) => isHiddenFromStudents({ status: e.phase, visible: e.visible, hidden_reason: e.hidden_reason })
const hiddenLabel = (e: ExamRow) => (e.hidden_reason ? `${pt('hubHidden')}: ${pt(`hubReason_${e.hidden_reason}` as PlatformKey)}` : pt('hubHidden'))
const ACTION_LABEL: Record<RowAction, PlatformKey> = {
  edit: 'exEdit', publish: 'exPublish', unpublish: 'exUnpublish', close: 'exClose', reopen: 'exReopen', archive: 'exArchive',
  restore: 'exRestore', duplicate: 'exDuplicate', delete: 'exDelete', results: 'exResults', grading: 'navGrading',
}
const actionLabel = (a: RowAction, e: ExamRow) => `${pt(ACTION_LABEL[a])}: ${e.title}`

/** Errors that mean the row is not in the state the page thought: the list is read again so the buttons match. */
const STALE = new Set(['has_attempts', 'locked', 'archived', 'invalid_transition', 'close_first', 'invalid_time'])

async function onAction(e: ExamRow, a: RowAction) {
  if (a === 'delete') {
    deleteError.value = ''
    deleting.value = e
    return
  }
  if (acting.value) return
  acting.value = e.id
  error.value = ''
  notice.value = ''
  try {
    const d = await teacherExams.action(e.id, a as ExamAction)
    if (a === 'duplicate') {
      await router.push(`/platform/exams/${d.id}/edit`)
      return
    }
    notice.value = pt('bankDone')
    await load()
  } catch (err) {
    error.value = platformErrorMessage(pt, err)
    if (err instanceof PlatformError && err.code === 'exams_disabled') refresh() // the numbers were stale: show the reason
    if (err instanceof PlatformError && STALE.has(err.code)) await load({ keepError: true })
  } finally {
    acting.value = ''
  }
}

async function confirmDelete() {
  const e = deleting.value
  if (!e || deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await teacherExams.remove(e.id)
    deleting.value = null
    notice.value = pt('bankDone')
    if (filters.page > 0 && items.value.length === 1) filters.page--
    await load()
  } catch (err) {
    if (err instanceof PlatformError && err.code === 'has_attempts') {
      // students took it meanwhile: deleting would erase their results. Say so, and show the row as it really is.
      deleting.value = null
      error.value = pt('mexDeleteBlocked')
      await load({ keepError: true })
    } else {
      deleteError.value = platformErrorMessage(pt, err)
    }
  } finally {
    deleteBusy.value = false
  }
}

onMounted(async () => {
  refresh()
  const [, subjects] = await Promise.allSettled([load(), myTeaching()])
  if (subjects.status === 'fulfilled') {
    teaching.value = subjects.value
    teachingKnown.value = true
  }
})
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="myexams-page">
    <router-link to="/platform/teacher" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <div class="flex items-start gap-3 flex-wrap">
      <div class="flex-1 min-w-0">
        <h1 class="text-display-sm font-bold tracking-tight">{{ pt('mexTitle') }}</h1>
        <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('mexSub') }}</p>
      </div>
      <router-link v-if="!examsOff && hasUsableSubject" to="/platform/exams/new" class="btn-filled" data-testid="myexams-new">{{ pt('exNew') }}</router-link>
      <button v-else-if="examsOff" type="button" class="btn-filled" disabled aria-describedby="myexams-off-reason" data-testid="myexams-new">{{ pt('exNew') }}</button>
    </div>
    <p v-if="examsOff" id="myexams-off-reason" class="card-filled p-3 text-body-md" role="status" data-testid="myexams-off-reason">{{ pt('examsOffReason') }}</p>

    <section v-if="!hasUsableSubject" class="card-filled p-4 space-y-2" aria-labelledby="myexams-need-subject-title" data-testid="myexams-need-subject">
      <h2 id="myexams-need-subject-title" class="text-title-md font-bold">{{ pt('hubNeedSubjectTitle') }}</h2>
      <p class="text-body-md">{{ pt('hubNeedSubject') }}</p>
      <router-link to="/platform/teaching" class="btn-filled inline-flex" data-testid="myexams-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </section>

    <form class="flex flex-wrap gap-2" role="search" :aria-label="pt('mexTitle')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" maxlength="100" dir="auto" class="input-outlined flex-1 min-w-[12rem]" :placeholder="pt('exSearch')" :aria-label="pt('exSearch')" data-testid="myexams-search" />
      <select v-model="filters.subjectId" class="input-outlined" :aria-label="pt('bankSubject')" data-testid="myexams-subject" @change="refilter">
        <option value="">{{ pt('exAllSubjects') }}</option>
        <option v-for="s in subjectOptions" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </form>

    <div class="flex flex-wrap gap-2" role="group" :aria-label="pt('mexTabs')">
      <button v-for="t in MY_TABS" :key="t" type="button" :class="filters.tab === t ? 'btn-filled' : 'btn-outlined'" :aria-pressed="filters.tab === t" :data-testid="`myexams-tab-${t}`" @click="setTab(t)">{{ pt(TAB_LABEL[t]) }}</button>
    </div>
    <p v-if="adminTab" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="myexams-admin-hint">{{ pt('mexAdminHint') }}</p>

    <p v-if="notice" role="status" class="text-body-md" data-testid="myexams-notice">{{ notice }}</p>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="myexams-error">{{ error }}</p>

    <div v-if="loaded && !loading && !items.length && !error" class="card-filled p-4 space-y-2" role="status" data-testid="myexams-empty">
      <template v-if="filtered && !adminTab">
        <p class="text-body-lg">{{ pt('exListEmpty') }}</p>
        <button type="button" class="btn-tonal" data-testid="myexams-clear" @click="clearFilters">{{ pt('hubClearFilters') }}</button>
      </template>
      <p v-else-if="adminTab" class="text-body-lg">{{ pt('mexEmptyAdmin') }}</p>
      <p v-else class="text-body-lg">{{ pt('mexEmpty') }}</p>
    </div>

    <ul class="space-y-3">
      <li v-for="e in items" :key="e.id" class="card-filled p-4 space-y-2" data-testid="myexam-row">
        <div class="flex items-start gap-3 flex-wrap">
          <div class="min-w-0 flex-1 space-y-0.5">
            <router-link :to="titleLink(e)" class="font-bold break-words" dir="auto" data-testid="myexam-title">{{ e.title }}</router-link>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              {{ e.subject_name }} · <span dir="ltr" class="inline-block">{{ e.question_count }}</span> {{ pt('exQuestions') }} · <span dir="ltr" class="inline-block">{{ e.total_points }}</span> {{ pt('exPoints') }}
              · <span dir="ltr" class="inline-block" data-testid="myexam-attempts">{{ e.attempt_count }}</span> {{ pt('exAttemptsCount') }}
            </div>
            <div v-if="e.opens_at || e.closes_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <template v-if="e.opens_at">{{ pt('exOpens').replace(/ \(.*\)/, '') }}: {{ fmt(e.opens_at) }}</template><template v-if="e.opens_at && e.closes_at"> · </template><template v-if="e.closes_at">{{ pt('exCloses').replace(/ \(.*\)/, '') }}: {{ fmt(e.closes_at) }}</template>
            </div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <span dir="ltr" class="inline-block font-bold" data-testid="myexam-submitted">{{ e.submitted }}</span> {{ pt('mexSubmitted') }}
              <template v-if="e.pending_answers > 0"> · <span dir="ltr" class="inline-block font-bold" data-testid="myexam-pending">{{ e.pending_answers }}</span> {{ pt('mexPendingAnswers') }}</template>
            </div>
            <div v-if="readOnlyRow(e)" class="text-body-sm font-semibold" data-testid="myexam-by-admin">{{ pt('mexByAdmin') }}</div>
            <p v-if="e.locked" class="text-body-sm font-semibold" data-testid="myexam-locked">{{ pt('mexLocked') }}</p>
          </div>
          <div class="flex flex-col items-end gap-1 shrink-0">
            <span class="text-xs font-semibold px-2 py-1 rounded-full" style="background-color: rgb(var(--md-surface-container-high))" data-testid="myexam-phase">{{ pt(`exPhase_${e.phase}` as PlatformKey) }}</span>
            <span v-if="isHidden(e)" class="text-xs font-semibold px-2 py-1 rounded-full break-words" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))" data-testid="myexam-hidden-chip">{{ hiddenLabel(e) }}</span>
          </div>
        </div>
        <HiddenWhy v-if="isHidden(e)" :reason="e.hidden_reason" />

        <div v-if="acts(e).shown.length" class="flex flex-wrap gap-1" role="group" :aria-label="e.title">
          <template v-for="a in acts(e).shown" :key="a">
            <router-link v-if="a === 'edit'" :to="`/platform/exams/${e.id}/edit`" class="btn-text" :aria-label="actionLabel(a, e)" data-testid="myexam-act-edit">{{ pt('exEdit') }}</router-link>
            <router-link v-else-if="a === 'results'" :to="`/platform/assessments/${e.id}/results`" class="btn-text" :aria-label="actionLabel(a, e)" data-testid="myexam-act-results">{{ pt('exResults') }}</router-link>
            <router-link v-else-if="a === 'grading'" :to="`/platform/grading/${e.id}`" class="btn-text" :aria-label="actionLabel(a, e)" data-testid="myexam-act-grading">{{ pt('navGrading') }}</router-link>
            <button
              v-else type="button" class="btn-text" :disabled="acting === e.id || acts(e).disabled.includes(a)"
              :aria-label="actionLabel(a, e)" :aria-describedby="acts(e).disabled.includes(a) ? 'myexams-off-reason' : undefined" :data-testid="`myexam-act-${a}`"
              @click="onAction(e, a)"
            >{{ pt(ACTION_LABEL[a]) }}</button>
          </template>
        </div>
        <p v-if="acts(e).deleteBlocked" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="myexam-delete-blocked">{{ pt('mexDeleteBlocked') }}</p>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="myexams-pager">
      <button type="button" class="btn-outlined" :disabled="filters.page === 0 || loading" data-testid="myexams-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ filters.page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('bankFound') }}</span>
      <button type="button" class="btn-outlined" :disabled="filters.page + 1 >= pages || loading" data-testid="myexams-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>

    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('mexDeleteTitle')}: ${deleting.title}`"
      :message="pt('exDeleteMsg')"
      :counts="[{ label: pt('questionsCount'), value: deleting.question_count }]"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="confirmDelete"
      @close="deleting = null"
    />
  </div>
</template>
