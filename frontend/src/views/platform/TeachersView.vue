<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { teachersDirectory, follow, unfollow, myEnrollments, type DirectoryTeacher, type EnrolledSubject } from '@/api/platformLearning'
import {
  DEFAULT_TEACHER_FILTERS, TEACHERS_PER, teacherClampPage, teacherFiltered, teacherFiltersFromQuery, teacherFiltersToQuery, teacherListQuery,
  teacherPages, withFollowing, type TeacherFilters,
} from '@/utils/teachersDirectory'
import PageLoading from '@/components/platform/PageLoading.vue'

// The teachers directory: searchable, paged (24 a page), and for a student narrowed to one of their own subjects. A student
// sees who they follow and can follow / unfollow from the list. The filters live in the address (back/forward and reloads
// keep them). "No teachers" is shown only once an answer is in - never while the list is loading or after an error.
const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()

const isStudent = computed(() => auth.role === 'student')
const filters = reactive<TeacherFilters>(teacherFiltersFromQuery(route.query))
const search = ref(filters.q)
const items = ref<DirectoryTeacher[]>([])
const total = ref(0)
const loading = ref(true)
const loaded = ref(false)
const error = ref('')
const followError = ref('')
const subjects = ref<EnrolledSubject[]>([])
const busyIds = ref<Set<string>>(new Set())
let seq = 0

const pages = computed(() => teacherPages(total.value))
const filtered = computed(() => teacherFiltered(filters))
// A subject (or its institution) that is switched off has no teachers to list: it is not offered, unless the address already
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
    const r = await teachersDirectory(teacherListQuery(filters, TEACHERS_PER))
    if (mine !== seq) return
    const page = teacherClampPage(filters.page, r.total)
    if (page !== filters.page && !r.items.length) {
      // the list shrank under us: go to the last page that exists
      filters.page = page
      syncQuery()
      return load()
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
  router.replace({ query: teacherFiltersToQuery(filters) }).catch(() => { /* a redundant navigation is not an error */ })
}
function refilter() {
  filters.page = 0
  syncQuery()
  void load()
}

// The address changed from outside (a link, back/forward): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/teachers') return
  const next = teacherFiltersFromQuery(route.query)
  if (JSON.stringify(teacherFiltersToQuery(next)) === JSON.stringify(teacherFiltersToQuery(filters))) return
  Object.assign(filters, next)
  search.value = next.q
  void load()
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
function clearFilters() {
  search.value = ''
  Object.assign(filters, DEFAULT_TEACHER_FILTERS)
  syncQuery()
  void load()
}
function go(delta: number) {
  filters.page = Math.max(0, filters.page + delta)
  syncQuery()
  void load()
}

async function toggleFollow(t: DirectoryTeacher) {
  if (busyIds.value.has(t.id)) return
  busyIds.value = new Set(busyIds.value).add(t.id)
  followError.value = ''
  const want = !t.following
  try {
    await (want ? follow(t.id) : unfollow(t.id))
    items.value = withFollowing(items.value, t.id, want)
  } catch (e) {
    followError.value = platformErrorMessage(pt, e)
  } finally {
    const rest = new Set(busyIds.value)
    rest.delete(t.id)
    busyIds.value = rest
  }
}

onMounted(() => {
  void load()
  // the subject filter lists the student's own subjects; without them the filter simply is not offered
  if (isStudent.value) myEnrollments().then((s) => { subjects.value = s }).catch(() => { /* optional filter */ })
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" data-testid="teachers-page">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('browseTeachers') }}</h1>
    <form class="mb-4 flex flex-wrap gap-2" role="search" :aria-label="pt('browseTeachers')" @submit.prevent="submitSearch">
      <input v-model="search" type="search" maxlength="100" dir="auto" :placeholder="pt('searchTeachers')" :aria-label="pt('searchTeachers')" class="input-outlined flex-1 min-w-[12rem]" data-testid="teachers-search" />
      <select
        v-if="isStudent && subjectOptions.length"
        v-model="filters.subjectId"
        class="input-outlined min-w-0 max-w-full"
        :aria-label="pt('tvSubjectFilter')"
        data-testid="teachers-subject"
        @change="refilter"
      >
        <option value="">{{ pt('tvAllMine') }}</option>
        <option v-for="s in subjectOptions" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </form>

    <div v-if="error" class="space-y-2 mb-3">
      <p class="text-body-md" role="alert" style="color: rgb(var(--md-error))" data-testid="teachers-error">{{ error }}</p>
      <button type="button" class="btn-tonal" data-testid="teachers-retry" @click="load()">{{ pt('retry') }}</button>
    </div>
    <p v-if="followError" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="teachers-follow-error">{{ followError }}</p>

    <PageLoading v-if="!loaded" :rows="2" />
    <div v-else-if="!loading && !error && !items.length" class="card-filled p-4 space-y-2" role="status" data-testid="teachers-empty">
      <p class="text-body-lg">{{ pt('noResults') }}</p>
      <button v-if="filtered" type="button" class="btn-tonal" data-testid="teachers-clear" @click="clearFilters">{{ pt('hubClearFilters') }}</button>
    </div>

    <ul class="space-y-3" :aria-busy="loading ? 'true' : undefined">
      <li v-for="t in items" :key="t.id" class="card-filled p-4 flex flex-wrap items-center gap-3" data-testid="teacher-row" :data-teacher-id="t.id">
        <router-link :to="`/platform/teachers/${t.id}`" class="flex-1 min-w-[10rem] no-underline" :data-testid="`teacher-link-${t.id}`">
          <span class="block font-bold break-words" dir="auto">{{ t.full_name }}</span>
          <span v-if="t.bio" class="block text-body-sm line-clamp-2" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ t.bio }}</span>
          <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ t.subject_count }}</span> {{ pt('tvSubjects') }}</span>
        </router-link>
        <template v-if="isStudent">
          <span
            v-if="t.following"
            class="text-xs font-semibold px-2 py-1 rounded-full"
            style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))"
            :data-testid="`teacher-following-${t.id}`"
          >{{ pt('tvFollowing') }}</span>
          <button
            type="button"
            :class="t.following ? 'btn-outlined' : 'btn-filled'"
            :disabled="busyIds.has(t.id)"
            :aria-label="`${t.following ? pt('unfollow') : pt('follow')}: ${t.full_name}`"
            :data-testid="`teacher-follow-${t.id}`"
            @click="toggleFollow(t)"
          >{{ t.following ? pt('unfollow') : pt('follow') }}</button>
        </template>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3 mt-4" :aria-label="pt('pageLabel')" data-testid="teachers-pager">
      <button type="button" class="btn-outlined" :disabled="filters.page === 0 || loading" data-testid="teachers-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ filters.page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('tvTotal') }}</span>
      <button type="button" class="btn-outlined" :disabled="filters.page + 1 >= pages || loading" data-testid="teachers-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
