<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { useTeacherStats } from '@/lib/teacherStats'
import { myContent, duplicatePost, duplicateCourse, type MyBundle } from '@/api/platformContent'
import { myAssessments, type AssessmentInfo } from '@/api/platformExams'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import {
  clampPage, copyTitle, filterExams, filtersFromQuery, filtersToQuery, hasActiveFilters, pageCount, pageOffset, sortExams, statusForTab,
  type HubFilters, type HubTab,
} from '@/utils/contentHub'
import { usableSubjects } from '@/utils/teachingCards'
import AssessmentList from '@/components/platform/AssessmentList.vue'
import ContentLists from '@/components/platform/ContentLists.vue'

// "My content": everything the teacher published or drafted, with search, filters, tabs, paging and a clear word for
// anything students cannot see ("hidden: <reason>"). Posts, courses and sessions are paged by the server; the exams list
// is small and unpaged, so it is filtered and paged here. The filters live in the address (back/forward and reloads keep them).
const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()
const PER = 20

const filters = reactive<HubFilters>(filtersFromQuery(route.query))
const search = ref(filters.q)
const bundle = ref<MyBundle>(emptyBundle())
const assessments = ref<AssessmentInfo[]>([])
const teaching = ref<MyTeaching[]>([])
const loading = ref(false)
const loaded = ref(false)
const examsLoaded = ref(false)
const teachingKnown = ref(false)
const error = ref('')
const copying = ref('')
let seq = 0

function emptyBundle(): MyBundle {
  return { posts: [], courses: [], live: [], totals: { posts: 0, courses: 0, live: 0 } }
}

// The admin can switch exam creation off; the button then says why instead of leading to an error.
const { stats, refresh } = useTeacherStats()
const examsOff = computed(() => stats.value?.can_create_exams === false)

// Creating anything needs an approved subject that is switched on. While the list is unknown the buttons stay available
// (the editors explain it themselves); once known with none, the page points at "My subjects".
const hasUsableSubject = computed(() => !teachingKnown.value || usableSubjects(teaching.value).length > 0)
const subjectOptions = computed(() => {
  const seen = new Map<string, string>()
  for (const t of teaching.value) if (!seen.has(t.subject_id)) seen.set(t.subject_id, t.subject_name)
  return [...seen].map(([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name))
})

const TAB_LABEL: Record<HubTab, PlatformKey> = { all: 'all', draft: 'statusDraft', published: 'statusPublished', hidden: 'hubTabHidden' }
const TABS: HubTab[] = ['all', 'draft', 'published', 'hidden']

const wantsServerLists = computed(() => filters.type !== 'exam')
const wantsExams = computed(() => filters.type === '' || filters.type === 'exam')

// Sorting is done where the paging is: by the server for posts, courses and sessions (its `sort` parameter, before it cuts
// the page) and here for the exams, over the whole filtered list before it is cut. A sort of only the visible page would
// shuffle each page on its own and lose the order between pages.
const filteredExams = computed(() =>
  wantsExams.value ? sortExams(filterExams(assessments.value, { q: filters.q, tab: filters.tab, subjectId: filters.subjectId }), filters.sort, i18n.locale) : [],
)
const shownExams = computed(() => filteredExams.value.slice(pageOffset(filters.page, PER), pageOffset(filters.page, PER) + PER))
const pages = computed(() => pageCount([bundle.value.totals.posts, bundle.value.totals.courses, bundle.value.totals.live, filteredExams.value.length], PER))

const nothingShown = computed(() => !bundle.value.posts.length && !bundle.value.courses.length && !bundle.value.live.length && !shownExams.value.length)
const filtered = computed(() => hasActiveFilters(filters))

async function loadLists() {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    if (!wantsServerLists.value) {
      bundle.value = emptyBundle()
    } else {
      const b = await myContent({
        q: filters.q.trim() || undefined,
        status: statusForTab(filters.tab),
        subject_id: filters.subjectId || undefined,
        type: filters.type === '' || filters.type === 'exam' ? undefined : filters.type,
        sort: filters.sort === 'title' ? 'title' : undefined,
        limit: PER,
        offset: pageOffset(filters.page, PER),
      })
      if (mine !== seq) return
      // The last item of the last page was just removed or moved to another tab: step back to a page that exists. The
      // exams share the pager, so they count too (page 2 may be empty for the server and still full of exams). While the
      // exams are not loaded yet nothing is decided here; `settlePage` runs again once they are.
      if (examsLoaded.value) {
        const page = clampPage(filters.page, [b.totals.posts, b.totals.courses, b.totals.live, filteredExams.value.length], PER)
        if (page !== filters.page) {
          filters.page = page
          syncQuery()
          loading.value = false
          return loadLists()
        }
      }
      bundle.value = b
    }
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === seq) {
      loading.value = false
      loaded.value = true
    }
  }
}

/** Once the exams are known: a page address that no list reaches (an old bookmark) moves to the last page that exists. */
async function settlePage() {
  const t = bundle.value.totals
  const page = clampPage(filters.page, [t.posts, t.courses, t.live, filteredExams.value.length], PER)
  if (page === filters.page) return
  filters.page = page
  syncQuery()
  if (wantsServerLists.value) await loadLists()
}

function syncQuery() {
  router.replace({ query: filtersToQuery(filters) }).catch(() => { /* a redundant navigation is not an error */ })
}

/** A filter changed: back to the first page, keep the address in step, fetch. */
function refilter() {
  filters.page = 0
  syncQuery()
  loadLists()
}

// The address changed from outside (a sidebar link back to "My content", the browser's back button): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/my-content') return // already leaving for another page
  const next = filtersFromQuery(route.query)
  if (JSON.stringify(filtersToQuery(next)) === JSON.stringify(filtersToQuery(filters))) return // our own update
  Object.assign(filters, next)
  search.value = next.q
  loadLists()
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
function setTab(t: HubTab) {
  if (filters.tab === t) return
  filters.tab = t
  refilter()
}
function clearFilters() {
  search.value = ''
  Object.assign(filters, { q: '', tab: 'all', type: '', subjectId: '', page: 0 })
  syncQuery()
  loadLists()
}
function go(delta: number) {
  filters.page = Math.max(0, filters.page + delta)
  syncQuery()
  if (wantsServerLists.value) loadLists()
}

async function duplicate(kind: 'post' | 'course', id: string, title: string) {
  if (copying.value) return
  copying.value = id
  error.value = ''
  try {
    const name = copyTitle(title, ` ${pt('hubCopySuffix')}`)
    const created = kind === 'post' ? await duplicatePost(id, name) : await duplicateCourse(id, name)
    await router.push(`/platform/${kind === 'post' ? 'posts' : 'courses'}/${created.id}/edit`)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    copying.value = ''
  }
}

onMounted(async () => {
  refresh()
  // everything is independent: one failure must not blank the others
  const [, exams, subjects] = await Promise.allSettled([loadLists(), myAssessments(), myTeaching()])
  if (exams.status === 'fulfilled') assessments.value = exams.value
  else if (!error.value) error.value = platformErrorMessage(pt, exams.reason)
  examsLoaded.value = true
  await settlePage()
  if (subjects.status === 'fulfilled') {
    teaching.value = subjects.value
    teachingKnown.value = true
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="content-page">
    <router-link to="/platform/teacher" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myContent') }}</h1>

    <div v-if="hasUsableSubject" class="flex flex-wrap gap-2" :class="examsOff ? 'mb-2' : 'mb-5'" role="group" :aria-label="pt('hubCreate')">
      <router-link to="/platform/posts/new" class="btn-filled" data-testid="new-post">{{ pt('newPost') }}</router-link>
      <router-link to="/platform/courses/new" class="btn-tonal" data-testid="new-course">{{ pt('newCourse') }}</router-link>
      <router-link to="/platform/live/new" class="btn-tonal" data-testid="new-live">{{ pt('newLive') }}</router-link>
      <router-link v-if="!examsOff" to="/platform/exams/new" class="btn-tonal" data-testid="new-exam">{{ pt('exNew') }}</router-link>
      <button v-else type="button" class="btn-tonal" disabled aria-describedby="exams-off-reason" data-testid="new-exam-disabled">{{ pt('exNew') }}</button>
    </div>
    <section v-else class="card-filled p-4 mb-5 space-y-2" aria-labelledby="content-need-subject-title" data-testid="content-need-subject">
      <h2 id="content-need-subject-title" class="text-title-md font-bold">{{ pt('hubNeedSubjectTitle') }}</h2>
      <p class="text-body-md">{{ pt('hubNeedSubject') }}</p>
      <router-link to="/platform/teaching" class="btn-filled inline-flex" data-testid="content-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </section>
    <p v-if="examsOff && hasUsableSubject" id="exams-off-reason" class="text-body-sm mb-5" style="color: rgb(var(--md-on-surface-variant))" data-testid="exams-off-reason">{{ pt('examsOffReason') }}</p>

    <form class="flex flex-wrap gap-2 mb-3" role="search" :aria-label="pt('myContent')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" maxlength="100" dir="auto" class="input-outlined flex-1 min-w-[12rem]" :placeholder="pt('hubSearch')" :aria-label="pt('hubSearch')" data-testid="content-search" />
      <select v-model="filters.type" class="input-outlined" :aria-label="pt('hubType')" data-testid="content-type" @change="refilter">
        <option value="">{{ pt('hubTypeAll') }}</option>
        <option value="post">{{ pt('posts') }}</option>
        <option value="course">{{ pt('courses') }}</option>
        <option value="live">{{ pt('liveSessions') }}</option>
        <option value="exam">{{ pt('assessments') }}</option>
      </select>
      <select v-model="filters.subjectId" class="input-outlined" :aria-label="pt('subject')" data-testid="content-subject" @change="refilter">
        <option value="">{{ pt('hubSubjectAll') }}</option>
        <option v-for="s in subjectOptions" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
      <select v-model="filters.sort" class="input-outlined" :aria-label="pt('hubSort')" data-testid="content-sort" @change="refilter">
        <option value="updated">{{ pt('hubSortUpdated') }}</option>
        <option value="title">{{ pt('hubSortTitle') }}</option>
      </select>
    </form>

    <div class="flex flex-wrap gap-2 mb-4" role="group" :aria-label="pt('hubTabs')">
      <button v-for="t in TABS" :key="t" type="button" :class="filters.tab === t ? 'btn-filled' : 'btn-outlined'" :aria-pressed="filters.tab === t" :data-testid="`content-tab-${t}`" @click="setTab(t)">{{ pt(TAB_LABEL[t]) }}</button>
    </div>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="content-error">{{ error }}</p>

    <div v-if="loaded && examsLoaded && !loading && nothingShown && !error" class="card-filled p-4 space-y-2" role="status" data-testid="content-empty">
      <template v-if="filtered">
        <p class="text-body-lg">{{ pt('hubNoMatch') }}</p>
        <button type="button" class="btn-tonal" data-testid="content-clear-filters" @click="clearFilters">{{ pt('hubClearFilters') }}</button>
      </template>
      <template v-else>
        <p class="text-body-lg">{{ pt('hubEmpty') }}</p>
        <p v-if="hasUsableSubject" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('hubEmptyHint') }}</p>
      </template>
    </div>

    <section v-if="shownExams.length" class="mb-6" data-testid="content-exams">
      <h2 class="text-title-md font-bold mb-2">{{ pt('assessments') }}</h2>
      <AssessmentList :items="shownExams" show-status show-edit own />
    </section>
    <ContentLists :bundle="bundle" show-status show-edit own hide-empty :busy-id="copying" @duplicate="duplicate" />

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3 mt-6" :aria-label="pt('pageLabel')" data-testid="content-pager">
      <button type="button" class="btn-outlined" :disabled="filters.page === 0 || loading" data-testid="content-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ filters.page + 1 }} / {{ pages }}</span></span>
      <button type="button" class="btn-outlined" :disabled="filters.page + 1 >= pages || loading" data-testid="content-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
