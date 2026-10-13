<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ArrowTopRightOnSquareIcon, CalendarDaysIcon, LockClosedIcon } from '@heroicons/vue/24/outline'
import { PlatformError } from '@/lib/platformApi'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { useRouteParam } from '@/composables/useRouteParam'
import { useEnrollAction } from '@/composables/useEnrollAction'
import { studentLive, type StudentLive } from '@/api/platformStudent'
import { canJoinNow, livePhase } from '@/utils/schedule'
import { countdown } from '@/utils/studentExams'
import { buildLiveIcs } from '@/utils/ics'
import { saveBlob } from '@/utils/saveFile'
import { parseHttpsUrl } from '@/utils/videoKind'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'

// A live lesson as a student sees it: when it is, how long, who gives it, a countdown to the moment joining opens (15
// minutes before the start), a Join button that works only inside the joining window, and "add to my calendar". The join
// link itself is withheld by the server from a student who is not enrolled in the subject (`locked`): the page then says so
// and offers to enrol. The phase and the joining window follow this device's clock (re-evaluated every second); the
// server only supplies the times.
const pt = usePt()
const i18n = useI18nStore()
const id = useRouteParam('id')
const enr = useEnrollAction()

const live = ref<StudentLive | null>(null)
const error = ref('')
const notFound = ref(false)
const now = ref(Date.now())
const calendarError = ref('')
let seq = 0

async function load(): Promise<void> {
  const mine = ++seq
  error.value = ''
  try {
    const l = await studentLive(id.value)
    if (mine !== seq) return
    live.value = l
    notFound.value = false
  } catch (e) {
    if (mine !== seq) return
    if (e instanceof PlatformError && (e.code === 'not_found' || e.status === 404)) notFound.value = true
    else error.value = platformErrorMessage(pt, e)
  }
}

// Another session (a link, a notification, back/forward) re-uses this page: forget the old one and load the new.
watch(id, () => {
  seq++
  live.value = null
  error.value = ''
  notFound.value = false
  calendarError.value = ''
  void load()
}, { immediate: true })

let tick: ReturnType<typeof setInterval> | undefined
onMounted(() => { tick = setInterval(() => { now.value = Date.now() }, 1000) })
onBeforeUnmount(() => clearInterval(tick))

const locale = computed(() => (i18n.locale === 'ar' ? 'ar' : undefined))
const fmt = (ms: number) => new Date(ms).toLocaleString(locale.value, { dateStyle: 'full', timeStyle: 'short' })

const endsAt = computed(() => (live.value ? live.value.starts_at + live.value.duration_min * 60_000 : 0))
const joinFrom = computed(() => live.value?.joinable_from ?? 0)
const cancelled = computed(() => live.value?.status === 'cancelled')
const phase = computed(() => (live.value ? livePhase(live.value.starts_at, endsAt.value, now.value) : 'upcoming'))
const joinOpen = computed(() => !!live.value && !cancelled.value && canJoinNow(live.value.starts_at, endsAt.value, now.value))
const untilJoin = computed(() => (live.value && !cancelled.value && now.value < joinFrom.value ? countdown(joinFrom.value, now.value) : null))
const joinHref = computed(() => (live.value && !live.value.locked ? parseHttpsUrl(live.value.join_url)?.href ?? null : null))
const two = (n: number) => String(n).padStart(2, '0')

const PHASE_LABEL: Record<'upcoming' | 'live_now' | 'ended', PlatformKey> = { upcoming: 'lvPhaseUpcoming', live_now: 'lvPhaseLive', ended: 'lvPhaseEnded' }
const PHASE_STYLE: Record<'upcoming' | 'live_now' | 'ended', string> = {
  upcoming: 'background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))',
  live_now: 'background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))',
  ended: 'background-color: rgb(var(--md-surface-container-high)); color: rgb(var(--md-on-surface-variant))',
}

/** Why the Join button cannot be pressed yet (the text next to it). */
const joinHint = computed<PlatformKey | null>(() => {
  if (!live.value || joinOpen.value) return null
  if (cancelled.value) return 'lvCancelled'
  if (phase.value === 'ended') return 'lvEndedHint'
  return 'lvJoinLater'
})

const joinTitleEl = ref<HTMLElement | null>(null)
const announce = ref('')

async function enrollNow() {
  if (!live.value) return
  const ok = await enr.run(live.value.subject_id, load)
  if (!ok) return
  // the Enroll button went away with the lock notice: say so, and put the focus on the section that just unlocked
  announce.value = ''
  await nextTick()
  announce.value = pt('lvEnrolledNow')
  joinTitleEl.value?.focus()
}

function addToCalendar() {
  if (!live.value) return
  calendarError.value = ''
  try {
    const l = live.value
    const text = buildLiveIcs(
      { id: l.id, title: l.title, description: l.description, teacher_name: l.teacher_name, subject_name: l.subject_name, starts_at: l.starts_at, duration_min: l.duration_min, join_url: l.join_url, locked: l.locked },
      { calName: pt('schCalName'), teacher: pt('lvTeacher'), subject: pt('lvSubject'), join: pt('lvJoinLink') },
    )
    saveBlob(new Blob([text], { type: 'text/calendar;charset=utf-8' }), `azlearn-live-${l.id}.ics`)
  } catch {
    calendarError.value = pt('genericError')
  }
}
</script>

<template>
  <div v-if="live" class="max-w-3xl mx-auto pb-8 space-y-5" data-testid="live-page">
    <p class="sr-only" role="status" data-testid="live-announce">{{ announce }}</p>
    <router-link to="/platform/schedule" class="text-body-sm underline">{{ pt('schTitle') }}</router-link>

    <div class="space-y-2">
      <h1 class="text-display-sm font-bold tracking-tight break-words" dir="auto" data-testid="live-title">{{ live.title }}</h1>
      <div class="flex flex-wrap items-center gap-2">
        <span class="text-sm font-semibold px-3 py-1 rounded-full" :style="PHASE_STYLE[phase]" data-testid="live-phase" :data-phase="phase">{{ pt(PHASE_LABEL[phase]) }}</span>
        <span v-if="cancelled" class="text-sm font-semibold px-3 py-1 rounded-full" style="background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))" data-testid="live-cancelled">{{ pt('cancelled') }}</span>
      </div>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">
        <router-link :to="`/platform/teachers/${live.teacher_id}`" class="underline" data-testid="live-teacher"><bdi>{{ live.teacher_name }}</bdi></router-link>
        · <router-link :to="`/platform/subjects/${live.subject_id}`" class="underline" data-testid="live-subject"><bdi>{{ live.subject_name }}</bdi></router-link>
      </p>
    </div>

    <section aria-labelledby="live-when-title" class="card-filled p-4 space-y-2">
      <h2 id="live-when-title" class="sr-only">{{ pt('lvDetails') }}</h2>
      <p class="font-semibold" data-testid="live-time"><bdi>{{ fmt(live.starts_at) }}</bdi></p>
      <p class="text-body-md"><span dir="ltr" class="inline-block" data-testid="live-duration">{{ live.duration_min }}</span> {{ pt('minutesShort') }}</p>
      <p v-if="live.description" class="text-body-lg whitespace-pre-line" dir="auto" data-testid="live-description">{{ live.description }}</p>
    </section>

    <section aria-labelledby="live-join-title" class="space-y-3">
      <h2 id="live-join-title" ref="joinTitleEl" tabindex="-1" class="text-title-md font-bold focus:outline-none">{{ pt('lvJoinTitle') }}</h2>

      <p v-if="untilJoin" class="text-body-md" role="timer" data-testid="live-countdown">
        {{ pt('lvOpensIn') }}:
        <template v-if="untilJoin.days"><span dir="ltr" class="inline-block font-bold">{{ untilJoin.days }}</span> {{ pt('stexDays') }} · </template>
        <span dir="ltr" class="inline-block font-bold tabular-nums">{{ two(untilJoin.hours) }}:{{ two(untilJoin.minutes) }}:{{ two(untilJoin.seconds) }}</span>
      </p>

      <div v-if="live.locked" class="card-filled p-4 space-y-3" data-testid="live-locked">
        <p class="flex items-center gap-2 font-semibold"><LockClosedIcon class="w-5 h-5 shrink-0" aria-hidden="true" /> <span>{{ pt('lvLocked') }}</span></p>
        <p class="text-body-sm">{{ pt('lvLockedWhy') }}</p>
        <button v-if="!live.enrolled" type="button" class="btn-filled" :disabled="enr.busy.value" data-testid="live-enroll" @click="enrollNow">{{ pt('enroll') }}</button>
        <p v-if="enr.error.value" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ enr.error.value }}</p>
      </div>

      <div v-else class="flex flex-wrap items-center gap-3">
        <a
          v-if="joinOpen && joinHref"
          :href="joinHref"
          target="_blank"
          rel="noopener noreferrer"
          class="btn-filled inline-flex items-center gap-2"
          data-testid="live-join"
          data-state="open"
        >{{ pt('schJoin') }}<ArrowTopRightOnSquareIcon class="w-4 h-4" aria-hidden="true" /><span class="sr-only">{{ pt('opensInNewTab') }}</span></a>
        <button v-else type="button" class="btn-filled opacity-60" disabled aria-disabled="true" data-testid="live-join" data-state="closed">{{ pt('schJoin') }}</button>
        <p v-if="joinHint" class="text-body-md flex-1 min-w-[10rem]" data-testid="live-join-hint">{{ pt(joinHint) }}</p>
        <p v-else-if="joinOpen && !joinHref" class="text-body-md flex-1 min-w-[10rem]" role="alert" data-testid="live-join-missing">{{ pt('lvNoLink') }}</p>
      </div>

      <p v-if="phase === 'ended' && !cancelled" class="text-body-md font-semibold" data-testid="live-ended">{{ pt('lvEndedNote') }}</p>

      <div v-if="phase !== 'ended' && !cancelled" class="flex flex-wrap items-center gap-2">
        <button type="button" class="btn-tonal inline-flex items-center gap-2" data-testid="live-calendar" @click="addToCalendar">
          <CalendarDaysIcon class="w-5 h-5" aria-hidden="true" />{{ pt('lvAddCalendar') }}
        </button>
        <p v-if="calendarError" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ calendarError }}</p>
      </div>
    </section>
  </div>
  <div v-else-if="notFound" class="max-w-3xl mx-auto space-y-3" data-testid="live-not-found">
    <router-link to="/platform/schedule" class="text-body-sm underline">{{ pt('schTitle') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('lvNotFoundTitle') }}</h1>
    <p class="text-body-lg">{{ pt('lvNotFoundBody') }}</p>
  </div>
  <PageError v-else-if="error" :message="error" back-to="/platform/schedule" />
  <PageLoading v-else />
</template>
