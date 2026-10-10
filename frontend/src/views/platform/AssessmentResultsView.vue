<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import {
  absentStudents, attemptsTable, downloadExport, examAnalytics, studentsTable,
  type AbsentStudent, type Analytics, type ResultFilter, type SortKey, type StudentResultFilter, type StudentRow, type StudentSortKey, type StudentSummary,
} from '@/api/platformGrading'
import PageError from '@/components/platform/PageError.vue'
import AnswerKeyDialog from '@/components/platform/AnswerKeyDialog.vue'

const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const id = route.params.id as string
const PAGE = 25

const a = ref<Analytics | null>(null)
const rows = ref<StudentRow[]>([])
// The table has two views of the same data: every attempt, or one row per student (best / last attempt, how many).
const view = ref<'attempts' | 'students'>('attempts')
const students = ref<StudentSummary[]>([])
const total = ref(0)
const page = ref(0)
const error = ref('')
const loadingRows = ref(false)
const search = ref('')
// Answer-key correction: only the exam's owner may (the server decides and says so in `can_correct`)
const canCorrect = ref(false)
const fixing = ref('')
const notice = ref('')
const f = reactive({ result: '' as ResultFilter, sort: 'submitted_at' as SortKey, dir: 'desc' as 'asc' | 'desc', q: '' })
const sf = reactive({ result: '' as StudentResultFilter, sort: 'name' as StudentSortKey, dir: 'asc' as 'asc' | 'desc', q: '' })

// Enrolled students with no attempt at all (names only), searched and paged on the server.
const ABSENT_PAGE = 25
// `loadedQ` is the search the shown rows answer (the box itself runs ahead of them while a request is on its way).
const absent = reactive({ items: [] as AbsentStudent[], total: 0, enrolled: 0, page: 0, q: '', loadedQ: '', loaded: false, error: '' })
const absentPages = computed(() => Math.max(1, Math.ceil(absent.total / ABSENT_PAGE)))

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const mins = (s: number | null) => (s === null ? '—' : `${Math.round((s / 60) * 10) / 10} ${pt('rsMinutes')}`)
const num = (n: number | null) => (n === null ? '—' : String(n))
const pages = computed(() => Math.max(1, Math.ceil(total.value / PAGE)))
const maxBin = computed(() => Math.max(1, ...(a.value?.distribution.map((b) => b.count) ?? [1])))

// Answers to the latest request only: a slow earlier answer must never replace the rows of a newer filter or view.
let rowsRequest = 0
async function loadRows() {
  const mine = ++rowsRequest
  loadingRows.value = true
  try {
    if (view.value === 'students') {
      const r = await studentsTable(id, { sort: sf.sort, dir: sf.dir, result: sf.result || undefined, q: sf.q || undefined, limit: PAGE, offset: page.value * PAGE })
      if (mine !== rowsRequest) return
      students.value = r.items
      total.value = r.total
    } else {
      const r = await attemptsTable(id, { sort: f.sort, dir: f.dir, result: f.result || undefined, q: f.q || undefined, limit: PAGE, offset: page.value * PAGE })
      if (mine !== rowsRequest) return
      rows.value = r.items
      total.value = r.total
    }
  } catch (e) {
    if (mine !== rowsRequest) return
    error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === rowsRequest) loadingRows.value = false
  }
}
const refilter = () => { page.value = 0; loadRows() }
const submitSearch = () => { f.q = search.value; sf.q = search.value; refilter() }
const go = (d: number) => { page.value += d; loadRows() }
function setView(v: 'attempts' | 'students') {
  if (view.value === v) return
  view.value = v
  page.value = 0
  total.value = 0
  loadRows()
}

let absentRequest = 0
async function loadAbsent() {
  const mine = ++absentRequest
  try {
    const asked = absent.q.trim()
    const r = await absentStudents(id, { q: asked || undefined, limit: ABSENT_PAGE, offset: absent.page * ABSENT_PAGE })
    if (mine !== absentRequest) return
    absent.loadedQ = asked
    absent.items = r.items
    absent.total = r.total
    absent.enrolled = r.enrolled
    absent.error = ''
  } catch {
    if (mine === absentRequest) absent.error = pt('rsAbsentError')
  } finally {
    if (mine === absentRequest) absent.loaded = true
  }
}
let absentTimer: ReturnType<typeof setTimeout> | undefined
/** Typing in the search box asks the server once the typing pauses. */
function onAbsentInput() {
  clearTimeout(absentTimer)
  absentTimer = setTimeout(() => { absent.page = 0; loadAbsent() }, 300)
}
function submitAbsent() {
  clearTimeout(absentTimer)
  absent.page = 0
  loadAbsent()
}
const goAbsent = (d: number) => { absent.page += d; loadAbsent() }
onBeforeUnmount(() => clearTimeout(absentTimer))

/** The page's numbers; the analytics also say whether the caller may correct the key (`can_correct`, decided by the server). */
async function loadAnalytics(): Promise<boolean> {
  try {
    const an = await examAnalytics(id)
    a.value = an
    canCorrect.value = an.can_correct === true
    return true
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    return false
  }
}

onMounted(async () => {
  if (!(await loadAnalytics())) return
  await Promise.all([loadRows(), loadAbsent()])
})

/** The key was corrected: every figure on the page changed, so all of it is read again. */
async function onApplied() {
  fixing.value = ''
  error.value = ''
  notice.value = pt('keyDone')
  if (await loadAnalytics()) await loadRows()
}

async function exportAs(format: 'xlsx' | 'csv', part: 'results' | 'questions') {
  error.value = ''
  try {
    await downloadExport(id, format, part, i18n.locale === 'ar' ? 'ar' : 'en')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

/** Result of a student's row: waiting for grading wins over a pass flag, which is only known once everything is graded. */
const studentStatus = (s: StudentSummary) =>
  s.pending ? `${pt('pendingGrading')} (${s.pending})` : s.passed === true ? pt('rsPassed') : s.passed === false ? pt('rsFailed') : s.best_attempt_id ? '' : pt('rsNoSubmitted')
const statusText = (r: StudentRow) =>
  r.status === 'expired' ? pt('rsExpired') : r.status === 'in_progress' ? pt('rsInProgress') : r.pending ? pt('pendingGrading') : r.passed === true ? pt('rsPassed') : r.passed === false ? pt('rsFailed') : ''
const typeLabel = (t: string) => pt(`bank_t_${t}` as PlatformKey)
const STUDENT_SORTS: { key: StudentSortKey; label: PlatformKey }[] = [
  { key: 'name', label: 'rsSortName' }, { key: 'best', label: 'rsSortBest' }, { key: 'attempts', label: 'rsSortAttempts' }, { key: 'last', label: 'rsSortLast' },
]
const SORTS: { key: SortKey; label: PlatformKey }[] = [
  { key: 'submitted_at', label: 'rsSortDate' }, { key: 'name', label: 'rsSortName' }, { key: 'score', label: 'rsSortScore' },
  { key: 'duration', label: 'rsSortDuration' }, { key: 'tab_leaves', label: 'rsSortLeaves' }, { key: 'status', label: 'rsSortStatus' },
]
</script>

<template>
  <div v-if="a" class="max-w-4xl mx-auto pb-12 space-y-6" data-testid="results-page">
    <div>
      <router-link :to="`/platform/assessments/${id}`" class="text-body-sm underline">{{ pt('back') }}</router-link>
      <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words">{{ a.info.title }} — {{ pt('results') }}</h1>
    </div>
    <p v-if="notice" class="text-body-sm font-semibold" role="status" data-testid="key-notice">{{ notice }}</p>
    <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="results-error">{{ error }}</p>

    <!-- headline numbers -->
    <section class="grid grid-cols-2 sm:grid-cols-4 gap-2 text-center" data-testid="tiles">
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsParticipants') }}</div><div class="font-bold" dir="ltr" data-testid="t-participants">{{ a.participants }} / {{ a.enrolled }}<span v-if="a.participation_pct !== null" class="text-body-sm font-normal"> ({{ a.participation_pct }}%)</span></div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('submittedCount') }}</div><div class="font-bold" data-testid="n-submitted">{{ a.attempts_submitted }}</div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsAverage') }}</div><div class="font-bold" dir="ltr" data-testid="avg">{{ num(a.average) }}<span v-if="a.average_pct !== null" class="text-body-sm font-normal"> ({{ a.average_pct }}%)</span></div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsMedian') }}</div><div class="font-bold" dir="ltr">{{ num(a.median) }}</div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsHighest') }}</div><div class="font-bold" dir="ltr" data-testid="t-highest">{{ num(a.highest) }}</div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsLowest') }}</div><div class="font-bold" dir="ltr">{{ num(a.lowest) }}</div></div>
      <div v-if="a.info.pass_mark" class="card-filled p-3"><div class="text-body-sm">{{ pt('rsPassFail') }}</div><div class="font-bold" dir="ltr" data-testid="t-pass">{{ a.passed }} / {{ a.failed }}</div></div>
      <div class="card-filled p-3"><div class="text-body-sm">{{ pt('rsAvgTime') }}</div><div class="font-bold" dir="ltr">{{ mins(a.avg_duration_sec) }}</div></div>
    </section>

    <router-link v-if="a.awaiting_grading || rows.some((r) => r.pending)" :to="`/platform/grading/${id}`" class="card-filled flex items-center justify-between gap-3 p-4 no-underline" data-testid="grade-link">
      <span class="font-bold">{{ pt('rsAwaiting') }}: <span dir="ltr" class="inline-block" data-testid="t-awaiting">{{ a.awaiting_grading }}</span> {{ pt('rsStudentsCount') }}</span>
      <span class="btn-filled">{{ pt('rsGradeNow') }}</span>
    </router-link>

    <!-- distribution: one series, one hue; counts printed on the bars so the numbers are readable without color -->
    <section class="space-y-2" data-testid="distribution">
      <h2 class="text-title-md font-bold">{{ pt('rsDistribution') }}</h2>
      <p v-if="!a.graded_students" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('rsNoData') }}</p>
      <ul v-else class="space-y-1.5" role="list">
        <li v-for="b in a.distribution" :key="b.from" class="flex items-center gap-3" :title="`${b.from}–${b.to}%: ${b.count}`" data-testid="bin">
          <span class="w-20 shrink-0 text-body-sm text-end" dir="ltr">{{ b.from }}–{{ b.to }}%</span>
          <span class="flex-1 h-4 rounded-sm" style="background-color: rgb(var(--md-surface-container-high))">
            <span class="block h-4" :style="{ width: (b.count / maxBin) * 100 + '%', backgroundColor: 'rgb(var(--md-primary))', borderRadius: '0 4px 4px 0', minWidth: b.count ? '4px' : '0' }"></span>
          </span>
          <span class="w-8 shrink-0 text-body-sm font-bold" dir="ltr" data-testid="bin-count">{{ b.count }}</span>
        </li>
      </ul>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('rsNote') }}</p>
    </section>

    <!-- per-question analysis -->
    <section class="space-y-2" data-testid="questions">
      <h2 class="text-title-md font-bold">{{ pt('rsQuestionAnalysis') }}</h2>
      <ul class="space-y-2">
        <li v-for="q in a.questions" :key="q.id" class="card-filled p-3 space-y-1" data-testid="q-row">
          <div class="flex items-start gap-2">
            <span dir="ltr" class="inline-block font-bold shrink-0">{{ q.position }}.</span>
            <span class="flex-1 min-w-0 break-words" dir="auto">{{ q.stem }}</span>
            <span v-if="q.voided" class="text-xs font-bold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-surface-container-high))" data-testid="flag-voided">⊘ {{ pt('keyVoided') }}</span>
            <span v-if="q.weak" class="text-xs font-bold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-error-container, var(--md-surface-container-high))); color: rgb(var(--md-on-error-container, var(--md-on-surface)))" data-testid="flag-weak">▼ {{ pt('rsWeak') }}</span>
            <span v-if="q.easy" class="text-xs font-bold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-surface-container-high))" data-testid="flag-easy">▲ {{ pt('rsEasy') }}</span>
          </div>
          <div class="flex items-center gap-3 text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            <span>{{ typeLabel(q.type) }}</span>
            <span class="flex-1 h-2 rounded-sm" style="background-color: rgb(var(--md-surface-container-high))" :title="q.rate === null || q.voided ? '' : `${Math.round(q.rate * 100)}%`">
              <span v-if="q.rate !== null && !q.voided" class="block h-2" :style="{ width: q.rate * 100 + '%', backgroundColor: 'rgb(var(--md-primary))', borderRadius: '0 4px 4px 0', minWidth: q.rate > 0 ? '4px' : '0' }"></span>
            </span>
            <span dir="ltr" class="shrink-0 font-bold" style="color: rgb(var(--md-on-surface))" data-testid="q-rate">{{ q.rate === null || q.voided ? '—' : Math.round(q.rate * 100) + '%' }}</span>
            <span dir="ltr" class="shrink-0">{{ q.graded }} {{ pt('rsGradedN') }}</span>
          </div>
          <div v-if="canCorrect" class="flex justify-end">
            <button type="button" class="btn-text" :aria-label="`${pt('keyFix')}: ${q.position}`" :data-testid="`key-fix-${q.id}`" @click="fixing = q.id">{{ pt('keyFix') }}</button>
          </div>
        </li>
      </ul>
    </section>

    <!-- students -->
    <section class="space-y-3" data-testid="students">
      <div class="flex items-center gap-2 flex-wrap">
        <h2 class="text-title-md font-bold flex-1">{{ pt('rsStudents') }}</h2>
        <div class="flex gap-1 flex-wrap" role="group" :aria-label="pt('rsExport')">
          <button class="btn-tonal" data-testid="export-xlsx" @click="exportAs('xlsx', 'results')">{{ pt('rsExportXlsx') }}</button>
          <button class="btn-outlined" data-testid="export-csv" @click="exportAs('csv', 'results')">{{ pt('rsExportCsvResults') }}</button>
          <button class="btn-outlined" data-testid="export-csv-q" @click="exportAs('csv', 'questions')">{{ pt('rsExportCsvQuestions') }}</button>
        </div>
      </div>
      <p v-if="a.tab_leaves_total" class="text-body-sm"><span data-testid="leaves-total">{{ pt('rsLeavesTotal') }}: <span dir="ltr" class="inline-block">{{ a.tab_leaves_total }}</span> ({{ a.tab_leave_students }} {{ pt('rsStudentsCount') }})</span></p>
      <div class="flex gap-1 flex-wrap" role="group" :aria-label="pt('rsViewLabel')">
        <button type="button" :class="view === 'attempts' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="view === 'attempts'" data-testid="tbl-view-attempts" @click="setView('attempts')">{{ pt('rsViewAttempts') }}</button>
        <button type="button" :class="view === 'students' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="view === 'students'" data-testid="tbl-view-students" @click="setView('students')">{{ pt('rsViewStudents') }}</button>
      </div>
      <form class="grid grid-cols-2 md:grid-cols-4 gap-2" @submit.prevent="submitSearch">
        <input v-model="search" type="search" :placeholder="pt('rsSearch')" :aria-label="pt('rsSearch')" class="input-outlined col-span-2 md:col-span-4" data-testid="tbl-search" />
        <template v-if="view === 'attempts'">
          <select v-model="f.result" class="input-outlined" :aria-label="pt('results')" data-testid="tbl-result" @change="refilter">
            <option value="">{{ pt('rsAllResults') }}</option><option value="passed">{{ pt('rsPassed') }}</option><option value="failed">{{ pt('rsFailed') }}</option>
            <option value="pending">{{ pt('rsPending') }}</option><option value="expired">{{ pt('rsExpired') }}</option><option value="in_progress">{{ pt('rsInProgress') }}</option>
          </select>
          <select v-model="f.sort" class="input-outlined" :aria-label="pt('rsSortBy')" data-testid="tbl-sort" @change="refilter">
            <option v-for="s in SORTS" :key="s.key" :value="s.key">{{ pt(s.label) }}</option>
          </select>
          <select v-model="f.dir" class="input-outlined" :aria-label="pt('rsSortDir')" data-testid="tbl-dir" @change="refilter"><option value="desc">{{ pt('rsDesc') }}</option><option value="asc">{{ pt('rsAsc') }}</option></select>
        </template>
        <template v-else>
          <select v-model="sf.result" class="input-outlined" :aria-label="pt('results')" data-testid="tbl-result" @change="refilter">
            <option value="">{{ pt('rsAllResults') }}</option><option value="passed">{{ pt('rsPassed') }}</option><option value="failed">{{ pt('rsFailed') }}</option>
            <option value="pending">{{ pt('rsPending') }}</option>
          </select>
          <select v-model="sf.sort" class="input-outlined" :aria-label="pt('rsSortBy')" data-testid="tbl-sort" @change="refilter">
            <option v-for="s in STUDENT_SORTS" :key="s.key" :value="s.key">{{ pt(s.label) }}</option>
          </select>
          <select v-model="sf.dir" class="input-outlined" :aria-label="pt('rsSortDir')" data-testid="tbl-dir" @change="refilter"><option value="asc">{{ pt('rsAsc') }}</option><option value="desc">{{ pt('rsDesc') }}</option></select>
        </template>
      </form>

      <template v-if="view === 'attempts'">
        <p v-if="!loadingRows && !rows.length" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="tbl-empty">{{ pt('rsNoAttempts') }}</p>
        <ul class="space-y-2">
          <li v-for="r in rows" :key="r.attempt_id" data-testid="tbl-row">
            <router-link :to="`/platform/attempts/${r.attempt_id}`" class="card-filled p-3 grid grid-cols-[1fr_auto] gap-x-3 gap-y-1 items-center no-underline">
              <span class="font-bold break-words min-w-0">{{ r.student_name }} <span v-if="r.attempt_no > 1" class="text-body-sm font-normal">({{ pt('rsAttemptNo') }} <span dir="ltr" class="inline-block">{{ r.attempt_no }}</span>)</span></span>
              <span class="font-bold" dir="ltr" data-testid="row-score">{{ r.status === 'submitted' ? `${r.score} / ${r.total}` : '' }}<template v-if="r.percent !== null"> ({{ r.percent }}%)</template></span>
              <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" dir="auto">
                <span data-testid="row-status" class="font-semibold" style="color: rgb(var(--md-on-surface))">{{ statusText(r) }}</span>
                <template v-if="r.duration_sec !== null"> · <span dir="ltr" class="inline-block">{{ mins(r.duration_sec) }}</span></template>
                <template v-if="r.submitted_at"> · {{ fmt(r.submitted_at) }}</template>
                <template v-if="r.email"> · <span dir="ltr" class="inline-block">{{ r.email }}</span></template>
              </span>
              <span v-if="r.tab_leaves > 0" class="text-body-sm" data-testid="row-leaves" :title="pt('tkTabLeaves')">↗ <span dir="ltr" class="inline-block">{{ r.tab_leaves }}</span></span>
            </router-link>
          </li>
        </ul>
      </template>
      <template v-else>
        <p v-if="!loadingRows && !students.length" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="tbl-empty">{{ pt('rsNoStudents') }}</p>
        <ul class="space-y-2">
          <li v-for="s in students" :key="s.student_id" class="card-filled p-3 space-y-1" data-testid="stu-row">
            <div class="flex items-start gap-3 flex-wrap">
              <span class="font-bold break-words min-w-0 flex-1">{{ s.student_name }}</span>
              <span class="font-bold" dir="ltr" data-testid="stu-best">{{ s.best_score === null ? '—' : `${s.best_score} / ${a.info.total_points}` }}<template v-if="s.best_percent !== null"> ({{ s.best_percent }}%)</template></span>
            </div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <span class="font-semibold" style="color: rgb(var(--md-on-surface))" data-testid="stu-status">{{ studentStatus(s) }}</span>
              <template v-if="studentStatus(s)"> · </template>{{ pt('rsStuAttempts') }}: <span dir="ltr" class="inline-block" data-testid="stu-attempts">{{ s.attempts }}</span>
              <template v-if="s.last_submitted_at"> · {{ pt('rsStuLast') }}: {{ fmt(s.last_submitted_at) }}</template>
              <template v-else-if="s.last_status === 'in_progress'"> · {{ pt('rsInProgress') }}</template>
              <template v-else-if="s.last_status === 'expired'"> · {{ pt('rsExpired') }}</template>
              <template v-if="s.tab_leaves > 0"> · <span data-testid="stu-leaves">↗ {{ pt('tkTabLeaves') }}: <span dir="ltr" class="inline-block">{{ s.tab_leaves }}</span></span></template>
              <template v-if="s.email"> · <span dir="ltr" class="inline-block">{{ s.email }}</span></template>
            </div>
            <div class="flex gap-2 flex-wrap">
              <router-link v-if="s.best_attempt_id" :to="`/platform/attempts/${s.best_attempt_id}`" class="btn-text" :aria-label="`${pt('rsBestAttempt')}: ${s.student_name}`" data-testid="stu-best-link">{{ pt('rsBestAttempt') }}</router-link>
              <router-link v-if="s.last_attempt_id && s.last_attempt_id !== s.best_attempt_id" :to="`/platform/attempts/${s.last_attempt_id}`" class="btn-text" :aria-label="`${pt('rsLastAttempt')}: ${s.student_name}`" data-testid="stu-last-link">{{ pt('rsLastAttempt') }}</router-link>
            </div>
          </li>
        </ul>
      </template>
      <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="tbl-pager">
        <button class="btn-outlined" :disabled="page === 0 || loadingRows" data-testid="tbl-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
        <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span></span>
        <button class="btn-outlined" :disabled="page + 1 >= pages || loadingRows" data-testid="tbl-next" @click="go(1)">{{ pt('nextPage') }}</button>
      </nav>
    </section>

    <!-- enrolled students who have not started the exam: names only -->
    <section class="space-y-3" aria-labelledby="absent-heading" data-testid="absent-section">
      <h2 id="absent-heading" class="text-title-md font-bold">{{ pt('rsAbsentTitle') }}</h2>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('rsAbsentIntro') }}</p>
      <p v-if="absent.error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="absent-error">{{ absent.error }}</p>
      <template v-else-if="absent.loaded">
        <p class="text-body-md"><span dir="ltr" class="inline-block font-bold" data-testid="absent-total">{{ absent.total }}</span> {{ pt('rsAbsentOf') }} <span dir="ltr" class="inline-block" data-testid="absent-enrolled">{{ absent.enrolled }}</span> {{ pt('rsStudentsCount') }}</p>
        <form v-if="absent.enrolled" @submit.prevent="submitAbsent">
          <input v-model="absent.q" type="search" :placeholder="pt('rsAbsentSearch')" :aria-label="pt('rsAbsentSearch')" class="input-outlined w-full" data-testid="absent-search" @input="onAbsentInput" />
        </form>
        <p v-if="!absent.items.length" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="absent-empty">{{ !absent.enrolled ? pt('rsAbsentNoEnrolled') : absent.loadedQ ? pt('rsAbsentNoMatch') : pt('rsAbsentNone') }}</p>
        <ul class="space-y-1">
          <li v-for="s in absent.items" :key="s.student_id" class="card-filled px-3 py-2 break-words" data-testid="absent-row">{{ s.student_name }}</li>
        </ul>
        <nav v-if="absent.total > ABSENT_PAGE" class="flex items-center justify-between gap-3" :aria-label="`${pt('rsAbsentTitle')} - ${pt('pageLabel')}`" data-testid="absent-pager">
          <button class="btn-outlined" :disabled="absent.page === 0" data-testid="absent-prev" @click="goAbsent(-1)">{{ pt('prevPage') }}</button>
          <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ absent.page + 1 }} / {{ absentPages }}</span></span>
          <button class="btn-outlined" :disabled="absent.page + 1 >= absentPages" data-testid="absent-next" @click="goAbsent(1)">{{ pt('nextPage') }}</button>
        </nav>
      </template>
    </section>
    <AnswerKeyDialog v-if="fixing" :exam-id="id" :question-id="fixing" @close="fixing = ''" @applied="onApplied" />
  </div>
  <PageError v-else-if="error" :message="error" />
</template>
