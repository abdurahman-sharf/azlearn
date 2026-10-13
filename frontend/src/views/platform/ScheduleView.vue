<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ArrowDownTrayIcon, CalendarDaysIcon, ClipboardDocumentCheckIcon, LockClosedIcon, VideoCameraIcon } from '@heroicons/vue/24/outline'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { downloadScheduleIcs, mySchedule, type ScheduleItem, type ScheduleKind } from '@/api/platformStudent'
import {
  groupByDay, isTodayWindow, itemEnd, safeLink, shiftWeek, weekFromQuery, weekToQuery, weekWindow, rowJoinAction,
} from '@/utils/schedule'
import PageLoading from '@/components/platform/PageLoading.vue'

// "My schedule": a week of live lessons (from my subjects and the teachers I follow) and the opening / closing times of
// my exams, grouped by day in my local time, with a button to download everything as a calendar file. The window is in the
// address (?start=YYYY-MM-DD, today when absent); "Join" appears while the joining window of a live row is open. The join
// link itself is never on this page: a row leads to the live page, which decides what I may see.
const pt = usePt()
const i18n = useI18nStore()
const route = useRoute()
const router = useRouter()

const now = ref(Date.now())
const start = ref(weekFromQuery(route.query, now.value))
const items = ref<ScheduleItem[]>([])
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const icsBusy = ref(false)
const icsError = ref('')
let seq = 0

const win = computed(() => weekWindow(start.value))
const days = computed(() => groupByDay(items.value, win.value.start))
const isToday = computed(() => isTodayWindow(start.value, now.value))
const locale = computed(() => (i18n.locale === 'ar' ? 'ar' : undefined))

const fmtDay = (ms: number) => new Date(ms).toLocaleDateString(locale.value, { weekday: 'long', day: 'numeric', month: 'long' })
const fmtShortDay = (ms: number) => new Date(ms).toLocaleDateString(locale.value, { day: 'numeric', month: 'short' })
const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString(locale.value, { hour: '2-digit', minute: '2-digit' })
const range = computed(() => `${fmtShortDay(win.value.start)} - ${fmtShortDay(win.value.end - 1)}`)

async function load(): Promise<void> {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const w = win.value
    const r = await mySchedule(w.from, w.to)
    if (mine !== seq) return
    items.value = r.items
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === seq) {
      loading.value = false
      loaded.value = true
    }
  }
}

function moveTo(ms: number) {
  start.value = ms
  router.replace({ query: weekToQuery(ms, Date.now()) }).catch(() => { /* a redundant navigation is not an error */ })
  load()
}
const prev = () => moveTo(shiftWeek(start.value, -1))
const next = () => moveTo(shiftWeek(start.value, 1))
const today = () => moveTo(weekFromQuery({}, Date.now()))

// The address changed from outside (back/forward, the sidebar link): follow it.
watch(() => route.fullPath, () => {
  if (route.path !== '/platform/schedule') return
  const wanted = weekFromQuery(route.query, Date.now())
  if (wanted === start.value) return
  start.value = wanted
  load()
})

async function downloadIcs() {
  if (icsBusy.value) return
  icsBusy.value = true
  icsError.value = ''
  try {
    await downloadScheduleIcs(i18n.locale === 'ar' ? 'ar' : 'en')
  } catch (e) {
    icsError.value = platformErrorMessage(pt, e)
  } finally {
    icsBusy.value = false
  }
}

const KIND_LABEL: Record<ScheduleKind, PlatformKey> = { live: 'schKindLive', exam_opens: 'schKindExamOpens', exam_closes: 'schKindExamCloses' }
const KIND_STYLE: Record<ScheduleKind, string> = {
  live: 'background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))',
  exam_opens: 'background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))',
  exam_closes: 'background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))',
}

// the "Join" label follows the clock: re-evaluate every 30 seconds while the page is open
let tick: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  load()
  tick = setInterval(() => { now.value = Date.now() }, 30_000)
})
onBeforeUnmount(() => clearInterval(tick))
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="schedule-page">
    <div class="flex flex-wrap items-start gap-3">
      <div class="flex-1 min-w-[12rem]">
        <h1 class="text-display-sm font-bold tracking-tight">{{ pt('schTitle') }}</h1>
        <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('schSub') }}</p>
      </div>
      <button type="button" class="btn-tonal inline-flex items-center gap-2" :disabled="icsBusy" data-testid="schedule-ics" @click="downloadIcs">
        <ArrowDownTrayIcon class="w-5 h-5" aria-hidden="true" />{{ pt('schDownloadIcs') }}
      </button>
    </div>
    <p v-if="icsError" role="alert" class="text-body-sm" style="color: rgb(var(--md-error))" data-testid="schedule-ics-error">{{ icsError }}</p>

    <div class="flex flex-wrap items-center gap-2" role="group" :aria-label="pt('schWeekNav')">
      <button type="button" class="btn-outlined" data-testid="schedule-prev" @click="prev">{{ pt('schPrevWeek') }}</button>
      <button type="button" class="btn-outlined" :disabled="isToday" data-testid="schedule-today" @click="today">{{ pt('schToday') }}</button>
      <button type="button" class="btn-outlined" data-testid="schedule-next" @click="next">{{ pt('schNextWeek') }}</button>
      <span class="ms-auto font-semibold" role="status" data-testid="schedule-range"><bdi>{{ range }}</bdi></span>
    </div>

    <div v-if="error" class="space-y-2">
      <p role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="schedule-error">{{ error }}</p>
      <button type="button" class="btn-tonal" data-testid="schedule-retry" @click="load()">{{ pt('retry') }}</button>
    </div>

    <PageLoading v-if="!loaded" :rows="3" />

    <div v-else-if="!loading && !items.length && !error" class="card-filled p-4 space-y-2" role="status" data-testid="schedule-empty">
      <p class="text-body-lg flex items-center gap-2"><CalendarDaysIcon class="w-6 h-6 shrink-0" aria-hidden="true" />{{ pt('schEmpty') }}</p>
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('schEmptyHint') }}</p>
      <div class="flex flex-wrap gap-2">
        <router-link to="/platform/my-subjects" class="btn-tonal" data-testid="schedule-go-subjects">{{ pt('mysubjTitle') }}</router-link>
        <router-link to="/platform/teachers" class="btn-outlined" data-testid="schedule-go-teachers">{{ pt('browseTeachers') }}</router-link>
      </div>
    </div>

    <div class="space-y-5" :aria-busy="loading ? 'true' : undefined">
      <section v-for="d in days" :key="d.key" :aria-labelledby="`sch-day-${d.key}`" data-testid="schedule-day" :data-day="d.key">
        <h2 :id="`sch-day-${d.key}`" class="text-title-md font-bold mb-2"><bdi>{{ fmtDay(d.start) }}</bdi></h2>
        <ul class="space-y-2">
          <li v-for="it in d.items" :key="`${it.kind}-${it.id}`" class="card-filled p-4 space-y-2" data-testid="schedule-item" :data-kind="it.kind">
            <div class="flex items-start gap-3 flex-wrap">
              <div class="min-w-0 flex-1 space-y-0.5">
                <span class="inline-flex items-center gap-1 text-xs font-semibold px-2 py-1 rounded-full mb-1" :style="KIND_STYLE[it.kind]" data-testid="schedule-kind">
                  <VideoCameraIcon v-if="it.kind === 'live'" class="w-4 h-4" aria-hidden="true" />
                  <ClipboardDocumentCheckIcon v-else class="w-4 h-4" aria-hidden="true" />
                  {{ pt(KIND_LABEL[it.kind]) }}
                </span>
                <div>
                  <router-link v-if="safeLink(it.link)" :to="safeLink(it.link)!" class="font-bold break-words" dir="auto" data-testid="schedule-title">{{ it.title }}</router-link>
                  <span v-else class="font-bold break-words" dir="auto" data-testid="schedule-title">{{ it.title }}</span>
                </div>
                <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
                  <span dir="ltr" class="inline-block" data-testid="schedule-time"><template v-if="it.kind === 'live'">{{ fmtTime(it.at) }} - {{ fmtTime(itemEnd(it)) }}</template><template v-else>{{ fmtTime(it.at) }}</template></span>
                  · <bdi>{{ it.subject_name }}</bdi><template v-if="it.teacher_name"> · <bdi>{{ it.teacher_name }}</bdi></template>
                  <template v-if="it.kind === 'live' && it.duration_min"> · <span dir="ltr" class="inline-block">{{ it.duration_min }}</span> {{ pt('minutesShort') }}</template>
                </div>
                <p v-if="it.kind === 'live' && it.locked" class="text-body-sm flex items-center gap-1 font-semibold" data-testid="schedule-locked">
                  <LockClosedIcon class="w-4 h-4 shrink-0" aria-hidden="true" />{{ pt('schLocked') }}
                </p>
              </div>
            </div>
            <div v-if="rowJoinAction(it, now) && safeLink(it.link)" class="flex flex-wrap gap-2">
              <router-link
                :to="safeLink(it.link)!"
                :class="rowJoinAction(it, now) === 'join' ? 'btn-filled' : 'btn-outlined'"
                :aria-label="`${rowJoinAction(it, now) === 'join' ? pt('schJoin') : pt('schDetails')}: ${it.title}`"
                data-testid="schedule-join"
                :data-mode="rowJoinAction(it, now)"
              >{{ rowJoinAction(it, now) === 'join' ? pt('schJoin') : pt('schDetails') }}</router-link>
            </div>
          </li>
        </ul>
      </section>
    </div>
  </div>
</template>
