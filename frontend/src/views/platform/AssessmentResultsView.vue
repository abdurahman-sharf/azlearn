<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { attemptsTable, downloadExport, examAnalytics, type Analytics, type ResultFilter, type SortKey, type StudentRow } from '@/api/platformGrading'

const pt = usePt()
const auth = useAuthStore()
const i18n = useI18nStore()
const route = useRoute()
const id = route.params.id as string
const PAGE = 25

const a = ref<Analytics | null>(null)
const rows = ref<StudentRow[]>([])
const total = ref(0)
const page = ref(0)
const error = ref('')
const loadingRows = ref(false)
const search = ref('')
const f = reactive({ result: '' as ResultFilter, sort: 'submitted_at' as SortKey, dir: 'desc' as 'asc' | 'desc', q: '' })

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const mins = (s: number | null) => (s === null ? '—' : `${Math.round((s / 60) * 10) / 10} ${pt('rsMinutes')}`)
const num = (n: number | null) => (n === null ? '—' : String(n))
const pages = computed(() => Math.max(1, Math.ceil(total.value / PAGE)))
const maxBin = computed(() => Math.max(1, ...(a.value?.distribution.map((b) => b.count) ?? [1])))

async function loadRows() {
  loadingRows.value = true
  try {
    const r = await attemptsTable(id, { sort: f.sort, dir: f.dir, result: f.result || undefined, q: f.q || undefined, limit: PAGE, offset: page.value * PAGE })
    rows.value = r.items
    total.value = r.total
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loadingRows.value = false
  }
}
const refilter = () => { page.value = 0; loadRows() }
const submitSearch = () => { f.q = search.value; refilter() }
const go = (d: number) => { page.value += d; loadRows() }

onMounted(async () => {
  try {
    a.value = await examAnalytics(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    return
  }
  await loadRows()
})

async function exportAs(format: 'xlsx' | 'csv', part: 'results' | 'questions') {
  error.value = ''
  try {
    await downloadExport(id, format, part, i18n.locale === 'ar' ? 'ar' : 'en')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

const statusText = (r: StudentRow) =>
  r.status === 'expired' ? pt('rsExpired') : r.status === 'in_progress' ? pt('rsInProgress') : r.pending ? pt('pendingGrading') : r.passed === true ? pt('rsPassed') : r.passed === false ? pt('rsFailed') : ''
const typeLabel = (t: string) => pt(`bank_t_${t}` as PlatformKey)
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
            <span v-if="q.weak" class="text-xs font-bold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-error-container, var(--md-surface-container-high))); color: rgb(var(--md-on-error-container, var(--md-on-surface)))" data-testid="flag-weak">▼ {{ pt('rsWeak') }}</span>
            <span v-if="q.easy" class="text-xs font-bold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-surface-container-high))" data-testid="flag-easy">▲ {{ pt('rsEasy') }}</span>
          </div>
          <div class="flex items-center gap-3 text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            <span>{{ typeLabel(q.type) }}</span>
            <span class="flex-1 h-2 rounded-sm" style="background-color: rgb(var(--md-surface-container-high))" :title="q.rate === null ? '' : `${Math.round(q.rate * 100)}%`">
              <span v-if="q.rate !== null" class="block h-2" :style="{ width: q.rate * 100 + '%', backgroundColor: 'rgb(var(--md-primary))', borderRadius: '0 4px 4px 0', minWidth: q.rate > 0 ? '4px' : '0' }"></span>
            </span>
            <span dir="ltr" class="shrink-0 font-bold" style="color: rgb(var(--md-on-surface))" data-testid="q-rate">{{ q.rate === null ? '—' : Math.round(q.rate * 100) + '%' }}</span>
            <span dir="ltr" class="shrink-0">{{ q.graded }} {{ pt('rsGradedN') }}</span>
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
      <form class="grid grid-cols-2 md:grid-cols-4 gap-2" @submit.prevent="submitSearch">
        <input v-model="search" type="search" :placeholder="pt('rsSearch')" class="input-outlined col-span-2 md:col-span-4" data-testid="tbl-search" />
        <select v-model="f.result" class="input-outlined" :aria-label="pt('results')" data-testid="tbl-result" @change="refilter">
          <option value="">{{ pt('rsAllResults') }}</option><option value="passed">{{ pt('rsPassed') }}</option><option value="failed">{{ pt('rsFailed') }}</option>
          <option value="pending">{{ pt('rsPending') }}</option><option value="expired">{{ pt('rsExpired') }}</option><option value="in_progress">{{ pt('rsInProgress') }}</option>
        </select>
        <select v-model="f.sort" class="input-outlined" :aria-label="pt('rsSortBy')" data-testid="tbl-sort" @change="refilter">
          <option v-for="s in SORTS" :key="s.key" :value="s.key">{{ pt(s.label) }}</option>
        </select>
        <select v-model="f.dir" class="input-outlined" :aria-label="pt('rsSortDir')" data-testid="tbl-dir" @change="refilter"><option value="desc">{{ pt('rsDesc') }}</option><option value="asc">{{ pt('rsAsc') }}</option></select>
      </form>
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
      <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="tbl-pager">
        <button class="btn-outlined" :disabled="page === 0 || loadingRows" data-testid="tbl-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
        <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span></span>
        <button class="btn-outlined" :disabled="page + 1 >= pages || loadingRows" data-testid="tbl-next" @click="go(1)">{{ pt('nextPage') }}</button>
      </nav>
    </section>
    <span v-if="auth.role === 'admin' && a.tab_leaves_total" class="text-body-sm" data-testid="leaves-total">{{ pt('rsLeavesTotal') }}: <span dir="ltr" class="inline-block">{{ a.tab_leaves_total }}</span> ({{ a.tab_leave_students }} {{ pt('rsStudentsCount') }})</span>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
