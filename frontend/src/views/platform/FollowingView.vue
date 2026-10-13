<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myFollowing, type BookmarkRow, type FollowedTeacher } from '@/api/platformStudent'
import { unfollow } from '@/api/platformLearning'
import { useBookmarks } from '@/lib/bookmarks'
import PageLoading from '@/components/platform/PageLoading.vue'

// "Following & saved": the teachers I follow (newest first) and the posts and courses I saved. Two tabs (kept in the
// address as ?tab=saved); the counts are shown on both tabs. A saved item that was hidden or deleted since is simply not
// listed (the server only returns what I can open now).
type Tab = 'teachers' | 'saved'
const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()
const bm = useBookmarks()

const tab = ref<Tab>(route.query.tab === 'saved' ? 'saved' : 'teachers')
const teachers = ref<FollowedTeacher[]>([])
const teachersLoaded = ref(false)
const teachersError = ref('')
const savedError = ref('')
const loadedSaved = ref(false)
const notice = ref('')
const busyIds = ref<Set<string>>(new Set())

const fmtDate = (ms: number) => new Date(ms).toLocaleDateString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium' })
const saved = computed(() => bm.rows.value)
const ready = computed(() => teachersLoaded.value && loadedSaved.value)

// The address changed from outside (the sidebar link back to the plain page, a link with ?tab=saved): follow it.
watch(() => route.query.tab, (v) => {
  if (route.path !== '/platform/following') return
  tab.value = v === 'saved' ? 'saved' : 'teachers'
})

function setTab(t: Tab) {
  if (tab.value === t) return
  tab.value = t
  router.replace({ query: t === 'saved' ? { tab: 'saved' } : {} }).catch(() => { /* a redundant navigation is not an error */ })
}

async function loadTeachers() {
  teachersError.value = ''
  try {
    teachers.value = (await myFollowing()).items
  } catch (e) {
    teachersError.value = platformErrorMessage(pt, e)
  } finally {
    teachersLoaded.value = true
  }
}
async function loadSaved() {
  savedError.value = ''
  await bm.refresh()
  if (bm.failed.value) savedError.value = pt('fllSavedError')
  loadedSaved.value = true
}
onMounted(() => { void loadTeachers(); void loadSaved() })

async function stopFollowing(t: FollowedTeacher) {
  if (busyIds.value.has(t.id)) return
  busyIds.value = new Set(busyIds.value).add(t.id)
  teachersError.value = ''
  notice.value = ''
  try {
    await unfollow(t.id)
    teachers.value = teachers.value.filter((x) => x.id !== t.id)
    notice.value = pt('fllUnfollowed')
  } catch (e) {
    teachersError.value = platformErrorMessage(pt, e)
  } finally {
    const rest = new Set(busyIds.value)
    rest.delete(t.id)
    busyIds.value = rest
  }
}

async function removeSaved(b: BookmarkRow) {
  savedError.value = ''
  notice.value = ''
  try {
    await bm.set(b.target_type, b.target_id, false)
    notice.value = pt('fllRemoved')
  } catch (e) {
    savedError.value = platformErrorMessage(pt, e)
  }
}

const itemLink = (b: BookmarkRow) => `/platform/${b.target_type === 'course' ? 'courses' : 'posts'}/${b.target_id}`
</script>

<template>
  <div class="max-w-3xl mx-auto pb-12 space-y-4" data-testid="following-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight">{{ pt('fllTitle') }}</h1>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('fllSub') }}</p>
    </div>

    <div class="flex flex-wrap gap-2" role="group" :aria-label="pt('fllTabs')">
      <button type="button" :class="tab === 'teachers' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="tab === 'teachers'" data-testid="following-tab-teachers" @click="setTab('teachers')">
        {{ pt('fllTeachers') }}
        <span v-if="teachersLoaded" class="ms-1 text-xs font-bold px-1.5 rounded-full" style="background-color: rgb(var(--md-surface-container-highest)); color: rgb(var(--md-on-surface))"><span dir="ltr" class="inline-block" data-testid="following-count-teachers">{{ teachers.length }}</span></span>
      </button>
      <button type="button" :class="tab === 'saved' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="tab === 'saved'" data-testid="following-tab-saved" @click="setTab('saved')">
        {{ pt('fllSaved') }}
        <span v-if="loadedSaved" class="ms-1 text-xs font-bold px-1.5 rounded-full" style="background-color: rgb(var(--md-surface-container-highest)); color: rgb(var(--md-on-surface))"><span dir="ltr" class="inline-block" data-testid="following-count-saved">{{ saved.length }}</span></span>
      </button>
    </div>

    <p v-if="notice" role="status" class="text-body-md font-semibold" style="color: rgb(var(--azl-success-text, var(--md-primary)))" data-testid="following-notice">{{ notice }}</p>

    <PageLoading v-if="!ready" :rows="3" />

    <section v-else-if="tab === 'teachers'" aria-labelledby="following-teachers-title" data-testid="following-teachers">
      <h2 id="following-teachers-title" class="sr-only">{{ pt('fllTeachers') }}</h2>
      <div v-if="teachersError" class="space-y-2 mb-3">
        <p role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="following-error">{{ teachersError }}</p>
        <button type="button" class="btn-tonal" data-testid="following-retry" @click="loadTeachers">{{ pt('retry') }}</button>
      </div>
      <div v-else-if="!teachers.length" class="card-filled p-4 space-y-2" role="status" data-testid="following-empty-teachers">
        <p class="text-body-lg">{{ pt('fllEmptyTeachers') }}</p>
        <router-link to="/platform/teachers" class="btn-tonal inline-flex" data-testid="following-go-teachers">{{ pt('browseTeachers') }}</router-link>
      </div>
      <ul v-else class="space-y-3">
        <li v-for="t in teachers" :key="t.id" class="card-filled p-4 flex flex-wrap items-center gap-3" data-testid="following-teacher" :data-teacher-id="t.id">
          <router-link :to="`/platform/teachers/${t.id}`" class="flex-1 min-w-[10rem] no-underline" :data-testid="`following-link-${t.id}`">
            <span class="block font-bold break-words" dir="auto">{{ t.full_name }}</span>
            <span v-if="t.bio" class="block text-body-sm line-clamp-2" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ t.bio }}</span>
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <span dir="ltr" class="inline-block">{{ t.subject_count }}</span> {{ pt('fllSubjects') }} · {{ pt('fllSince') }} {{ fmtDate(t.followed_at) }}
            </span>
          </router-link>
          <button
            type="button"
            class="btn-outlined"
            :disabled="busyIds.has(t.id)"
            :aria-label="`${pt('unfollow')}: ${t.full_name}`"
            :data-testid="`following-unfollow-${t.id}`"
            @click="stopFollowing(t)"
          >{{ pt('unfollow') }}</button>
        </li>
      </ul>
    </section>

    <section v-else aria-labelledby="following-saved-title" data-testid="following-saved">
      <h2 id="following-saved-title" class="sr-only">{{ pt('fllSaved') }}</h2>
      <div v-if="savedError" class="space-y-2 mb-3">
        <p role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="saved-error">{{ savedError }}</p>
        <button type="button" class="btn-tonal" data-testid="saved-retry" @click="loadSaved">{{ pt('retry') }}</button>
      </div>
      <div v-else-if="!saved.length" class="card-filled p-4 space-y-2" role="status" data-testid="saved-empty">
        <p class="text-body-lg">{{ pt('fllEmptySaved') }}</p>
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('fllEmptySavedHint') }}</p>
        <router-link to="/platform/learning" class="btn-tonal inline-flex" data-testid="saved-go-learning">{{ pt('lrnTitle') }}</router-link>
      </div>
      <ul v-else class="space-y-3">
        <li v-for="b in saved" :key="`${b.target_type}:${b.target_id}`" class="card-filled p-4 flex flex-wrap items-center gap-3" data-testid="saved-row" :data-kind="b.target_type">
          <div class="flex-1 min-w-[10rem] space-y-0.5">
            <span class="inline-block text-xs font-semibold px-2 py-0.5 rounded-full mb-1" style="background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface))" data-testid="saved-kind">{{ b.target_type === 'course' ? pt('kindCourse') : pt('kindPost') }}</span>
            <div><router-link :to="itemLink(b)" class="font-bold break-words" dir="auto" data-testid="saved-title">{{ b.title }}</router-link></div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <bdi>{{ b.subject_name }}</bdi> · <bdi>{{ b.teacher_name }}</bdi> · {{ pt('fllSavedOn') }} {{ fmtDate(b.saved_at) }}
            </div>
          </div>
          <button
            type="button"
            class="btn-outlined"
            :aria-label="`${pt('fllRemove')}: ${b.title}`"
            :data-testid="`saved-remove-${b.target_id}`"
            @click="removeSaved(b)"
          >{{ pt('fllRemove') }}</button>
        </li>
      </ul>
    </section>
  </div>
</template>
