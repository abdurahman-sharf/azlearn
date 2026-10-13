<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { search, type Hit, type SearchResults } from '@/api/platformOps'
import {
  DEBOUNCE_MS, FIRST_PAGE, MORE_PAGE, SEARCH_GROUPS, groupsFor, hasMoreHits, highlightParts, mergeHits, normalizeQuery, queryValid, safeHitLink, totalCount,
  type SearchGroup,
} from '@/utils/searchGroups'
import PageLoading from '@/components/platform/PageLoading.vue'

// Platform search. Nothing is sent before two characters (a hint shows instead); typing waits 300 ms; every group of
// results has its own section with the total number of matches and, when there are more than listed, a "Show all" button
// that fetches the rest of THAT group only. The matched words are highlighted by splitting the text into pieces
// (never HTML), and every hit links to the in-app path the server gave it. Exams are searched for students only.
const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()

const q = ref(typeof route.query.q === 'string' ? route.query.q : '')
/** the normalized query the shown results belong to ('' = none) */
const active = ref('')
const hits = ref<Record<SearchGroup, Hit[]>>(emptyHits())
const counts = ref<Record<SearchGroup, number>>(emptyCounts())
const error = ref('')
const loading = ref(false)
const answered = ref(false)
const moreBusy = ref<Record<SearchGroup, boolean>>(emptyFlags())
const moreError = ref<Record<SearchGroup, boolean>>(emptyFlags())
let seq = 0

function emptyHits(): Record<SearchGroup, Hit[]> {
  return { subjects: [], teachers: [], courses: [], posts: [], live: [], exams: [] }
}
function emptyCounts(): Record<SearchGroup, number> {
  return { subjects: 0, teachers: 0, courses: 0, posts: 0, live: 0, exams: 0 }
}
function emptyFlags(): Record<SearchGroup, boolean> {
  return { subjects: false, teachers: false, courses: false, posts: false, live: false, exams: false }
}

const GROUP_LABEL: Record<SearchGroup, PlatformKey> = {
  subjects: 'subjectsLabel', teachers: 'teachersLabel', courses: 'coursesLabel', posts: 'postsLabel', live: 'srchLiveLabel', exams: 'srchExamsLabel',
}
const groups = computed(() => groupsFor(auth.role))
const valid = computed(() => queryValid(q.value))
const total = computed(() => totalCount(counts.value, groups.value))
const visibleGroups = computed(() => groups.value.filter((g) => hits.value[g].length > 0))

function reset() {
  seq++
  active.value = ''
  hits.value = emptyHits()
  counts.value = emptyCounts()
  error.value = ''
  loading.value = false
  answered.value = false
  moreBusy.value = emptyFlags()
  moreError.value = emptyFlags()
}

/** The server's answer as one array per group and one count per group (an older server may not send live / exams / counts). */
function shape(r: SearchResults): { hits: Record<SearchGroup, Hit[]>; counts: Record<SearchGroup, number> } {
  const h = emptyHits()
  const c = emptyCounts()
  for (const g of SEARCH_GROUPS) {
    h[g] = r[g] ?? []
    c[g] = Math.max(r.counts?.[g] ?? 0, h[g].length)
  }
  return { hits: h, counts: c }
}

async function run() {
  const text = normalizeQuery(q.value)
  if (!queryValid(text)) {
    reset()
    syncQuery('')
    return
  }
  const mine = ++seq
  error.value = ''
  loading.value = true
  moreBusy.value = emptyFlags()
  moreError.value = emptyFlags()
  syncQuery(text)
  try {
    const r = await search(text)
    if (mine !== seq) return
    const s = shape(r)
    hits.value = s.hits
    counts.value = s.counts
    active.value = text
    answered.value = true
  } catch (e) {
    if (mine !== seq) return
    error.value = platformErrorMessage(pt, e)
    hits.value = emptyHits()
    counts.value = emptyCounts()
    answered.value = false
  } finally {
    if (mine === seq) loading.value = false
  }
}

/** "Show all" of one group: the next page of that group's matches, appended below the ones already listed. */
async function showMore(g: SearchGroup) {
  if (moreBusy.value[g] || !active.value) return
  const mine = seq
  moreBusy.value = { ...moreBusy.value, [g]: true }
  moreError.value = { ...moreError.value, [g]: false }
  try {
    const r = await search(active.value, { group: g, limit: MORE_PAGE, offset: hits.value[g].length })
    if (mine !== seq) return
    const s = shape(r)
    hits.value = { ...hits.value, [g]: mergeHits(hits.value[g], s.hits[g]) }
    counts.value = { ...counts.value, [g]: Math.max(s.counts[g], hits.value[g].length) }
  } catch {
    if (mine === seq) moreError.value = { ...moreError.value, [g]: true }
  } finally {
    if (mine === seq) moreBusy.value = { ...moreBusy.value, [g]: false }
  }
}

function syncQuery(text: string) {
  const now = typeof route.query.q === 'string' ? route.query.q : ''
  if (now === text) return
  router.replace({ query: text ? { q: text } : {} }).catch(() => { /* a redundant navigation is not an error */ })
}

let timer: ReturnType<typeof setTimeout> | undefined
watch(q, (v) => {
  clearTimeout(timer)
  if (!queryValid(v)) {
    // too short: no request, the hint shows, and whatever was listed goes away at once
    reset()
    return
  }
  if (normalizeQuery(v) === active.value) {
    // back to the query whose results are on screen (a backspace after a typo): a request for the longer text may still be
    // in flight - drop its answer, so the list, the counts, the highlights and the address are the input's again
    seq++
    loading.value = false
    moreBusy.value = emptyFlags()
    syncQuery(active.value)
    return
  }
  timer = setTimeout(run, DEBOUNCE_MS)
})
onBeforeUnmount(() => clearTimeout(timer))

// The address changed from outside (the sidebar link, a link with ?q=): follow it.
watch(() => route.query.q, (v) => {
  if (route.path !== '/platform/search') return
  const next = typeof v === 'string' ? v : ''
  if (normalizeQuery(next) !== normalizeQuery(q.value)) q.value = next
})

function submit() {
  clearTimeout(timer)
  void run()
}

onMounted(() => { if (valid.value) void run() })

const summary = computed(() => {
  if (!valid.value) return pt('srchHint')
  if (loading.value && !answered.value) return pt('srchLoading')
  return ''
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="search-page">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('searchTitle') }}</h1>
    <form class="mb-3" role="search" :aria-label="pt('searchTitle')" @submit.prevent="submit">
      <input v-model="q" type="search" maxlength="60" dir="auto" :placeholder="pt('searchPlaceholder')" :aria-label="pt('searchPlaceholder')" class="input-outlined w-full" data-testid="search-input" />
    </form>

    <p class="text-body-md mb-3" role="status" style="color: rgb(var(--md-on-surface-variant))" data-testid="search-summary">
      <template v-if="summary">{{ summary }}</template>
      <template v-else-if="answered && !error">
        <template v-if="total > 0">{{ pt('srchSummary') }}: <span dir="ltr" class="inline-block font-semibold" data-testid="search-total">{{ total }}</span></template>
        <template v-else>{{ pt('noResults') }}</template>
      </template>
    </p>

    <div v-if="error" class="space-y-2 mb-3">
      <p class="text-body-md" role="alert" style="color: rgb(var(--md-error))" data-testid="search-error">{{ error }}</p>
      <button type="button" class="btn-tonal" data-testid="search-retry" @click="run()">{{ pt('retry') }}</button>
    </div>

    <PageLoading v-if="loading && !answered" :rows="2" />

    <div :aria-busy="loading ? 'true' : undefined">
      <section
        v-for="g in visibleGroups"
        :key="g"
        :aria-labelledby="`search-title-${g}`"
        class="mb-5"
        :data-testid="`search-group-${g}`"
      >
        <h2 :id="`search-title-${g}`" class="text-title-md font-bold mb-2">
          {{ pt(GROUP_LABEL[g]) }} <span class="text-body-md font-normal" style="color: rgb(var(--md-on-surface-variant))">(<span dir="ltr" class="inline-block" :data-testid="`search-count-${g}`">{{ counts[g] }}</span>)</span>
        </h2>
        <ul class="space-y-2">
          <li v-for="h in hits[g]" :key="h.id">
            <component :is="safeHitLink(h.link) ? RouterLink : 'div'" :to="safeHitLink(h.link) ?? undefined" class="card-filled block p-3 no-underline" data-testid="search-hit" :data-group="g">
              <div class="font-bold break-words" dir="auto" data-testid="search-hit-title"><template v-for="(p, i) in highlightParts(h.title, active)" :key="i"><mark v-if="p.match" class="search-mark">{{ p.text }}</mark><template v-else>{{ p.text }}</template></template></div>
              <div v-if="h.subtitle" class="text-body-sm truncate" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ h.subtitle }}</div>
              <div v-if="h.snippet" class="text-body-sm line-clamp-2 break-words" dir="auto" data-testid="search-hit-snippet"><template v-for="(p, i) in highlightParts(h.snippet, active)" :key="i"><mark v-if="p.match" class="search-mark">{{ p.text }}</mark><template v-else>{{ p.text }}</template></template></div>
            </component>
          </li>
        </ul>
        <div v-if="hasMoreHits(counts[g], hits[g].length)" class="mt-2 flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="btn-tonal"
            :disabled="moreBusy[g]"
            :aria-label="`${pt('srchShowAll')}: ${pt(GROUP_LABEL[g])}`"
            :data-testid="`search-more-${g}`"
            @click="showMore(g)"
          >{{ moreBusy[g] ? pt('srchLoading') : pt('srchShowAll') }}</button>
          <p v-if="moreError[g]" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" :data-testid="`search-more-error-${g}`">{{ pt('srchMoreFailed') }}</p>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
/* the matched part: a background AND bold, so it never depends on colour alone */
.search-mark {
  background-color: rgb(var(--azl-amber));
  color: rgb(var(--azl-on-amber));
  font-weight: 700;
  border-radius: 0.25rem;
  padding: 0 0.125rem;
}
</style>
