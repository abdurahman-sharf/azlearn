<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myExams, type StudentExamCounts, type StudentExamRow, type StudentExamTab } from '@/api/platformStudent'
import { myEnrollments, type EnrolledSubject } from '@/api/platformLearning'
import {
  DEFAULT_STEX_FILTERS, STEX_PER, STUDENT_TABS, bestView, fmtScore, reasonKey, rowLinks, runningButUnreachable, stateChip, stexClampPage, stexFiltersFromQuery,
  stexFiltersToQuery, stexListQuery, tabCount, type StateChip, type StudentExamFilters,
} from '@/utils/studentExams'
import PageLoading from '@/components/platform/PageLoading.vue'

// "My exams" for a student: every exam of my subjects with what it is for me (open now, upcoming, in progress, done,
// missed), why I cannot start it, my attempts and my best result - plus the exams I took that are no longer shown (only
// so I can reach my own results). The state is the server's; the filters live in the address (back/forward and reloads
// keep them).
const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()

const filters = reactive<StudentExamFilters>(stexFiltersFromQuery(route.query))
const search = ref(filters.q)
const items = ref<StudentExamRow[]>([])
const total = ref(0)
const counts = ref<StudentExamCounts | null>(null)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const subjects = ref<EnrolledSubject[]>([])
let seq = 0

const TAB_LABEL: Record<StudentExamTab, PlatformKey> = {
  all: 'exAll', open: 'stexState_open', upcoming: 'stexState_upcoming', in_progress: 'stexState_in_progress', done: 'stexState_done', missed: 'stexState_missed',
}
const pages = computed(() => Math.max(1, Math.ceil(total.value / STEX_PER)))
const filtered = computed(() => !!filters.q.trim() || filters.tab !== 'all' || !!filters.subjectId)
const subjectOptions = computed(() =>
  [...subjects.value].map((s) => ({ id: s.subject_id, name: s.subject_name })).sort((a, b) => a.name.localeCompare(b.name)),
)

async function load(): Promise<void> {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const r = await myExams(stexListQuery(filters, STEX_PER))
    if (mine !== seq) return
    const page = stexClampPage(filters.page, r.total)
    if (page !== filters.page && !r.items.length) {
      filters.page = page
      syncQuery()
      loading.value = false
      return load()
    }
    items.value = r.items
    total.value = r.total
    counts.value = r.counts
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
  router.replace({ query: stexFiltersToQuery(filters) }).catch(() => { /* a redundant navigation is not an error */ })
}
function refilter() {
  filters.page = 0
  syncQuery()
  load()
}

// The address changed from outside (the sidebar link, an overview card, back/forward): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/my-exams') return
  const next = stexFiltersFromQuery(route.query)
  if (JSON.stringify(stexFiltersToQuery(next)) === JSON.stringify(stexFiltersToQuery(filters))) return
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
function setTab(t: StudentExamTab) {
  if (filters.tab === t) return
  filters.tab = t
  refilter()
}
function clearFilters() {
  search.value = ''
  Object.assign(filters, DEFAULT_STEX_FILTERS)
  syncQuery()
  load()
}
function go(delta: number) {
  filters.page = Math.max(0, filters.page + delta)
  syncQuery()
  load()
}

const fmt = (ms: number | null) => (ms ? new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '')
/** the chip of a row: an open exam the student is not enrolled in reads "Not enrolled", never "Missed" */
const chipOf = (r: StudentExamRow): StateChip => stateChip(r.state, r.reason) ?? r.state
const stateLabel = (r: StudentExamRow) => pt(`stexState_${chipOf(r)}` as PlatformKey)
const reasonText = (r: StudentExamRow) => {
  const k = r.state === 'in_progress' ? null : reasonKey(r.reason)
  return k ? pt(k as PlatformKey) : ''
}
/** chip colours: never the only signal (the chip always carries the state's name) */
const STATE_STYLE: Record<StateChip, string> = {
  in_progress: 'background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))',
  open: 'background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))',
  upcoming: 'background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))',
  done: 'background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface))',
  missed: 'background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface-variant))',
  not_enrolled: 'background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface-variant))',
}

onMounted(async () => {
  await Promise.allSettled([
    load(),
    myEnrollments().then((s) => { subjects.value = s }),
  ])
})
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="stexams-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight">{{ pt('stexTitle') }}</h1>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stexSub') }}</p>
    </div>

    <form class="flex flex-wrap gap-2" role="search" :aria-label="pt('stexTitle')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" maxlength="100" dir="auto" class="input-outlined flex-1 min-w-[12rem]" :placeholder="pt('exSearch')" :aria-label="pt('exSearch')" data-testid="stexams-search" />
      <select v-model="filters.subjectId" class="input-outlined min-w-0 max-w-full" :aria-label="pt('bankSubject')" data-testid="stexams-subject" @change="refilter">
        <option value="">{{ pt('exAllSubjects') }}</option>
        <option v-for="s in subjectOptions" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </form>

    <div class="flex flex-wrap gap-2" role="group" :aria-label="pt('stexTabs')">
      <button
        v-for="t in STUDENT_TABS" :key="t" type="button"
        :class="filters.tab === t ? 'btn-filled' : 'btn-outlined'" :aria-pressed="filters.tab === t" :data-testid="`stexams-tab-${t}`"
        @click="setTab(t)"
      >
        {{ pt(TAB_LABEL[t]) }}
        <span v-if="counts" class="ms-1 text-xs font-bold px-1.5 rounded-full" style="background-color: rgb(var(--md-surface-container-highest)); color: rgb(var(--md-on-surface))"><span dir="ltr" class="inline-block" :data-testid="`stexams-count-${t}`">{{ tabCount(t, counts) }}</span></span>
      </button>
    </div>

    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="stexams-error">{{ error }}</p>

    <PageLoading v-if="!loaded" :rows="3" />

    <div v-else-if="!loading && !items.length && !error" class="card-filled p-4 space-y-2" role="status" data-testid="stexams-empty">
      <template v-if="filtered">
        <p class="text-body-lg">{{ pt('exListEmpty') }}</p>
        <button type="button" class="btn-tonal" data-testid="stexams-clear" @click="clearFilters">{{ pt('hubClearFilters') }}</button>
      </template>
      <template v-else>
        <p class="text-body-lg">{{ pt('stexEmpty') }}</p>
        <router-link to="/platform/my-subjects" class="btn-tonal inline-flex" data-testid="stexams-go-subjects">{{ pt('mysubjTitle') }}</router-link>
      </template>
    </div>

    <ul class="space-y-3" :aria-busy="loading ? 'true' : undefined">
      <li v-for="e in items" :key="e.id" class="card-filled p-4 space-y-2" data-testid="stexam-row" :data-state="e.state" :data-exam-id="e.id">
        <div class="flex items-start gap-3 flex-wrap">
          <div class="min-w-0 flex-1 space-y-0.5">
            <router-link v-if="e.visible" :to="`/platform/assessments/${e.id}`" class="font-bold break-words" dir="auto" data-testid="stexam-title">{{ e.title }}</router-link>
            <span v-else class="font-bold break-words" dir="auto" data-testid="stexam-title">{{ e.title }}</span>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <bdi>{{ e.subject_name }}</bdi> · <bdi>{{ e.teacher_name }}</bdi> · <span dir="ltr" class="inline-block">{{ e.question_count }}</span> {{ pt('exQuestions') }}
              · <span dir="ltr" class="inline-block">{{ fmtScore(e.total_points) }}</span> {{ pt('exPoints') }}
              <template v-if="e.duration_min"> · <span dir="ltr" class="inline-block">{{ e.duration_min }}</span> {{ pt('minutesShort') }}</template>
            </div>
            <div v-if="e.opens_at || e.closes_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <template v-if="e.opens_at">{{ pt('stuOpensAt') }}: {{ fmt(e.opens_at) }}</template><template v-if="e.opens_at && e.closes_at"> · </template><template v-if="e.closes_at">{{ pt('stuClosesAt') }}: {{ fmt(e.closes_at) }}</template>
            </div>
            <div class="text-body-sm">
              {{ pt('attemptsUsed') }}: <span dir="ltr" class="inline-block" data-testid="stexam-attempts">{{ e.attempts_used }} / {{ e.max_attempts }}</span>
              <template v-if="e.pass_mark"> · {{ pt('tkPassMark') }}: <span dir="ltr" class="inline-block">{{ e.pass_mark }}%</span></template>
            </div>
            <div v-if="bestView(e.best).kind !== 'none'" class="text-body-sm" data-testid="stexam-best">
              {{ pt('stexBest') }}:
              <template v-if="bestView(e.best).kind === 'withheld'"><span class="font-semibold">{{ pt('tkResultAfterClose') }}</span></template>
              <template v-else-if="e.best && e.best.score !== null">
                <span dir="ltr" class="inline-block font-bold">{{ fmtScore(e.best.score) }} / {{ fmtScore(e.best.total) }}</span>
                <template v-if="e.best.percent !== null"> (<span dir="ltr" class="inline-block">{{ Math.round(e.best.percent) }}%</span>)</template>
                <span v-if="e.best.passed !== null" class="ms-1 font-semibold">· {{ e.best.passed ? pt('tkPassed') : pt('tkFailed') }}</span>
                <span v-if="(e.best.pending ?? 0) > 0" class="ms-1" data-testid="stexam-best-pending">· {{ pt('pendingGrading') }}</span>
              </template>
            </div>
            <p v-if="e.state === 'in_progress' && e.in_progress_attempt" class="text-body-sm font-semibold" data-testid="stexam-ends">{{ pt('stuResumeEnds') }}: {{ fmt(e.in_progress_attempt.ends_at) }}</p>
            <p v-if="reasonText(e)" class="text-body-sm" data-testid="stexam-reason">{{ reasonText(e) }}</p>
            <p v-if="!e.visible" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="stexam-hidden">{{ pt('stexHiddenNote') }}</p>
            <p v-if="runningButUnreachable(e)" class="text-body-sm font-semibold" data-testid="stexam-run-hidden">{{ pt('stexRunHidden') }}</p>
          </div>
          <span class="shrink-0 text-xs font-semibold px-2 py-1 rounded-full" :style="STATE_STYLE[chipOf(e)]" data-testid="stexam-state">{{ stateLabel(e) }}</span>
        </div>

        <div v-if="rowLinks(e).start || rowLinks(e).resume || rowLinks(e).result" class="flex flex-wrap gap-2" role="group" :aria-label="e.title">
          <router-link v-if="rowLinks(e).resume" :to="rowLinks(e).resume!" class="btn-filled" :aria-label="`${pt('resumeAssessment')}: ${e.title}`" data-testid="stexam-resume">{{ pt('resumeAssessment') }}</router-link>
          <router-link v-if="rowLinks(e).start" :to="rowLinks(e).start!" class="btn-filled" :aria-label="`${pt('stexStart')}: ${e.title}`" data-testid="stexam-start">{{ pt('stexStart') }}</router-link>
          <router-link v-if="rowLinks(e).result" :to="rowLinks(e).result!" class="btn-outlined" :aria-label="`${pt('stexResult')}: ${e.title}`" data-testid="stexam-result">{{ pt('stexResult') }}</router-link>
        </div>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="stexams-pager">
      <button type="button" class="btn-outlined" :disabled="filters.page === 0 || loading" data-testid="stexams-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ filters.page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('bankFound') }}</span>
      <button type="button" class="btn-outlined" :disabled="filters.page + 1 >= pages || loading" data-testid="stexams-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
