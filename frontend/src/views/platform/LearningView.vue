<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myLearning, type LearningCounts, type LearningItem, type LearningTab } from '@/api/platformStudent'
import { myEnrollments, type EnrolledSubject } from '@/api/platformLearning'
import {
  DEFAULT_LEARNING_FILTERS, LEARNING_PER, LEARNING_TABS, continueLink, learningClampPage, learningFiltersFromQuery, learningFiltersToQuery,
  learningListQuery, learningTabCount, progressView, rowAction, type LearningFilters,
} from '@/utils/learning'
import PageLoading from '@/components/platform/PageLoading.vue'

// "My learning": every course of the subjects I am enrolled in, with how far I am and a button that continues where I
// stopped (the next lesson I have not completed). Tabs by state, a search box and a subject filter; all of it lives in
// the address (back/forward and reloads keep it), and the numbers and states are the server's.
const pt = usePt()
const route = useRoute()
const router = useRouter()

const filters = reactive<LearningFilters>(learningFiltersFromQuery(route.query))
const search = ref(filters.q)
const items = ref<LearningItem[]>([])
const total = ref(0)
const counts = ref<LearningCounts | null>(null)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const subjects = ref<EnrolledSubject[]>([])
let seq = 0

const TAB_LABEL: Record<LearningTab, PlatformKey> = {
  all: 'exAll', in_progress: 'lrnState_in_progress', not_started: 'lrnState_not_started', completed: 'lrnState_completed',
}
const pages = computed(() => Math.max(1, Math.ceil(total.value / LEARNING_PER)))
const filtered = computed(() => !!filters.q.trim() || filters.tab !== 'all' || !!filters.subjectId)
// A subject (or its institution) that is switched off has no courses to list: it is not offered, unless the address already
// names it (so the select does not lose its value).
const subjectOptions = computed(() =>
  subjects.value
    .filter((s) => s.active !== false || s.subject_id === filters.subjectId)
    .map((s) => ({ id: s.subject_id, name: s.subject_name }))
    .sort((a, b) => a.name.localeCompare(b.name)),
)

async function load(): Promise<void> {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const r = await myLearning(learningListQuery(filters, LEARNING_PER))
    if (mine !== seq) return
    const page = learningClampPage(filters.page, r.total)
    if (page !== filters.page && !r.items.length) {
      // the list shrank under us (a course was hidden, a filter narrowed it): go to the last page that exists
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
  router.replace({ query: learningFiltersToQuery(filters) }).catch(() => { /* a redundant navigation is not an error */ })
}
function refilter() {
  filters.page = 0
  syncQuery()
  load()
}

// The address changed from outside (the sidebar link, an overview link, back/forward): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/learning') return
  const next = learningFiltersFromQuery(route.query)
  if (JSON.stringify(learningFiltersToQuery(next)) === JSON.stringify(learningFiltersToQuery(filters))) return
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
function setTab(t: LearningTab) {
  if (filters.tab === t) return
  filters.tab = t
  refilter()
}
function clearFilters() {
  search.value = ''
  Object.assign(filters, DEFAULT_LEARNING_FILTERS)
  syncQuery()
  load()
}
function go(delta: number) {
  filters.page = Math.max(0, filters.page + delta)
  syncQuery()
  load()
}

const ACTION_LABEL: Record<'review' | 'continue' | 'start', PlatformKey> = { review: 'lrnReview', continue: 'lrnContinue', start: 'lrnStart' }
/** chip styles: never the only signal (every chip carries the state's name) */
const STATE_STYLE: Record<LearningItem['state'], string> = {
  in_progress: 'background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))',
  not_started: 'background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface))',
  completed: 'background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))',
}

onMounted(async () => {
  await Promise.allSettled([
    load(),
    myEnrollments().then((s) => { subjects.value = s }),
  ])
})
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="learning-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight">{{ pt('lrnTitle') }}</h1>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('lrnSub') }}</p>
    </div>

    <form class="flex flex-wrap gap-2" role="search" :aria-label="pt('lrnTitle')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" maxlength="100" dir="auto" class="input-outlined flex-1 min-w-[12rem]" :placeholder="pt('lrnSearch')" :aria-label="pt('lrnSearch')" data-testid="learn-search" />
      <select v-model="filters.subjectId" class="input-outlined min-w-0 max-w-full" :aria-label="pt('bankSubject')" data-testid="learn-subject" @change="refilter">
        <option value="">{{ pt('exAllSubjects') }}</option>
        <option v-for="s in subjectOptions" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </form>

    <div class="flex flex-wrap gap-2" role="group" :aria-label="pt('lrnTabs')">
      <button
        v-for="t in LEARNING_TABS" :key="t" type="button"
        :class="filters.tab === t ? 'btn-filled' : 'btn-outlined'" :aria-pressed="filters.tab === t" :data-testid="`learn-tab-${t}`"
        @click="setTab(t)"
      >
        {{ pt(TAB_LABEL[t]) }}
        <span v-if="counts" class="ms-1 text-xs font-bold px-1.5 rounded-full" style="background-color: rgb(var(--md-surface-container-highest)); color: rgb(var(--md-on-surface))"><span dir="ltr" class="inline-block" :data-testid="`learn-count-${t}`">{{ learningTabCount(t, counts) }}</span></span>
      </button>
    </div>

    <div v-if="error" class="space-y-2">
      <p role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="learn-error">{{ error }}</p>
      <button type="button" class="btn-tonal" data-testid="learn-retry" @click="load()">{{ pt('retry') }}</button>
    </div>

    <PageLoading v-if="!loaded" :rows="3" />

    <div v-else-if="!loading && !items.length && !error" class="card-filled p-4 space-y-2" role="status" data-testid="learn-empty">
      <template v-if="filtered">
        <p class="text-body-lg">{{ pt('lrnNoMatch') }}</p>
        <button type="button" class="btn-tonal" data-testid="learn-clear" @click="clearFilters">{{ pt('hubClearFilters') }}</button>
      </template>
      <template v-else>
        <p class="text-body-lg">{{ pt('lrnEmpty') }}</p>
        <router-link to="/platform/my-subjects" class="btn-tonal inline-flex" data-testid="learn-go-subjects">{{ pt('mysubjTitle') }}</router-link>
      </template>
    </div>

    <ul class="space-y-3" :aria-busy="loading ? 'true' : undefined">
      <li v-for="c in items" :key="c.course_id" class="card-filled p-4 space-y-2" data-testid="learn-row" :data-state="c.state" :data-course-id="c.course_id">
        <div class="flex items-start gap-3 flex-wrap">
          <div class="min-w-0 flex-1 space-y-0.5">
            <router-link :to="`/platform/courses/${c.course_id}`" class="font-bold break-words" dir="auto" data-testid="learn-title">{{ c.title }}</router-link>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <bdi>{{ c.subject_name }}</bdi> · <bdi>{{ c.teacher_name }}</bdi>
            </div>
            <p v-if="c.description" class="text-body-sm line-clamp-2" dir="auto">{{ c.description }}</p>
          </div>
          <span class="shrink-0 text-xs font-semibold px-2 py-1 rounded-full" :style="STATE_STYLE[c.state]" data-testid="learn-state">{{ pt(`lrnState_${c.state}` as PlatformKey) }}</span>
        </div>

        <div>
          <div class="flex justify-between text-body-sm mb-1">
            <span>{{ pt('progress') }}</span>
            <span><span dir="ltr" class="inline-block" data-testid="learn-count">{{ progressView(c).label }}</span> · <span dir="ltr" class="inline-block" data-testid="learn-percent">{{ progressView(c).percent }}%</span></span>
          </div>
          <div
            class="h-2 rounded-full overflow-hidden"
            style="background-color: rgb(var(--md-surface-container-high))"
            role="progressbar"
            :aria-label="`${pt('progress')}: ${c.title}`"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="progressView(c).percent"
            :aria-valuetext="progressView(c).label"
            data-testid="learn-bar"
          >
            <div class="h-full" :style="{ width: progressView(c).percent + '%', backgroundColor: 'rgb(var(--md-primary))' }"></div>
          </div>
        </div>

        <div class="flex flex-wrap gap-2">
          <router-link
            v-if="rowAction(c) === 'review'"
            :to="`/platform/courses/${c.course_id}`"
            class="btn-outlined"
            :aria-label="`${pt(ACTION_LABEL.review)}: ${c.title}`"
            data-testid="learn-review"
          >{{ pt(ACTION_LABEL.review) }}</router-link>
          <router-link
            v-else
            :to="continueLink(c)"
            :class="rowAction(c) === 'continue' ? 'btn-filled' : 'btn-tonal'"
            :aria-label="`${pt(ACTION_LABEL[rowAction(c)])}: ${c.title}`"
            data-testid="learn-continue"
          >{{ pt(ACTION_LABEL[rowAction(c)]) }}</router-link>
        </div>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="learn-pager">
      <button type="button" class="btn-outlined" :disabled="filters.page === 0 || loading" data-testid="learn-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ filters.page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('bankFound') }}</span>
      <button type="button" class="btn-outlined" :disabled="filters.page + 1 >= pages || loading" data-testid="learn-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
