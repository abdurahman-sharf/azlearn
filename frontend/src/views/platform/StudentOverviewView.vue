<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { CheckCircleIcon, ClockIcon, PlayCircleIcon, VideoCameraIcon } from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { useI18nStore } from '@/stores/i18n'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useStudentStats } from '@/lib/studentStats'
import { feed, type Bundle } from '@/api/platformContent'
import { fmtScore, overviewEmpty, overviewSteps, pct, showOverviewChecklist, type OverviewStep } from '@/utils/studentExams'
import ContentLists from '@/components/platform/ContentLists.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import PageError from '@/components/platform/PageError.vue'

// The student's home: an attempt to finish first, then what needs doing (exams closing soon, results waiting for a
// grade), the latest results, courses in progress, the next live lesson and - for a new account - the first steps
// (choose my level, enrol in subjects). Every number comes from one request (`/student/overview`, shared with the
// sidebar badges); the latest content of my subjects is a second, optional one.
const pt = usePt()
const auth = useAuthStore()
const i18n = useI18nStore()
const { stats, failed, refresh } = useStudentStats()
const settled = ref(false)
const latest = ref<Bundle | null>(null)

onMounted(async () => {
  await refresh()
  settled.value = true
  feed().then((b) => { latest.value = b }).catch(() => { /* optional section: hidden when it cannot be read */ })
})

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const ov = computed(() => stats.value)
const steps = computed(() => overviewSteps(ov.value))
const showChecklist = computed(() => showOverviewChecklist(steps.value))
const empty = computed(() => overviewEmpty(ov.value))
const STEP_LABEL: Record<OverviewStep, PlatformKey> = { placement: 'stuStepPlacement', enrol: 'stuStepEnrol' }
const STEP_TESTID: Record<OverviewStep, string> = { placement: 'ov-checklist-placement', enrol: 'ov-checklist-enrol' }
const feedHasItems = computed(() => !!latest.value && (latest.value.posts.length > 0 || latest.value.courses.length > 0 || latest.value.live.length > 0))
</script>

<template>
  <div v-if="ov" class="max-w-5xl mx-auto pb-8 space-y-6" data-testid="student-overview">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('stuOverview') }}</h1>
      <p class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('welcome') }} <bdi>{{ auth.profile?.full_name }}</bdi></p>
    </div>
    <p v-if="failed" class="text-body-md" role="alert" style="color: rgb(var(--md-error))" data-testid="ov-stale">{{ pt('stuOverviewStale') }}</p>

    <!-- an attempt that is still running comes first: its time is ticking -->
    <section v-if="ov.resume" aria-labelledby="ov-resume-title">
      <h2 id="ov-resume-title" class="sr-only">{{ pt('stuResumeTitle') }}</h2>
      <router-link
        :to="`/platform/assessments/${ov.resume.assessment_id}/take`"
        class="card-elevated flex flex-col sm:flex-row sm:items-center gap-3 p-5 no-underline"
        style="border-inline-start: 6px solid rgb(var(--azl-amber))"
        data-testid="ov-resume"
      >
        <PlayCircleIcon class="w-10 h-10 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
        <span class="flex-1 min-w-0">
          <span class="block text-title-md font-bold">{{ pt('stuResumeTitle') }}</span>
          <span class="block font-semibold break-words" dir="auto" data-testid="ov-resume-title">{{ ov.resume.title }}</span>
          <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stuResumeEnds') }}: {{ fmt(ov.resume.ends_at) }}</span>
        </span>
        <span class="btn-filled shrink-0">{{ pt('resumeAssessment') }}</span>
      </router-link>
    </section>

    <!-- first steps of a new account: choose my level, then enrol in subjects -->
    <section v-if="showChecklist" aria-labelledby="ov-checklist-title" data-testid="ov-checklist">
      <h2 id="ov-checklist-title" class="text-title-md font-bold mb-1">{{ pt('stuChecklistTitle') }}</h2>
      <p class="text-body-md mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stuChecklistBody') }}</p>
      <ol class="space-y-2">
        <li v-for="s in steps" :key="s.id">
          <router-link to="/platform/my-subjects" :data-testid="STEP_TESTID[s.id]" :data-done="s.done ? 'true' : 'false'" class="card-filled flex items-center gap-3 p-4 no-underline">
            <CheckCircleIcon v-if="s.done" class="w-6 h-6 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
            <span v-else class="w-6 h-6 shrink-0 rounded-full border-2" style="border-color: rgb(var(--md-outline))" aria-hidden="true"></span>
            <span class="flex-1 font-bold">{{ pt(STEP_LABEL[s.id]) }}</span>
            <span class="text-body-sm font-semibold">{{ s.done ? pt('tchStepDone') : pt('tchStepTodo') }}</span>
          </router-link>
        </li>
      </ol>
    </section>

    <!-- exams: the ones closing soon, then how many are open / upcoming -->
    <section aria-labelledby="ov-exams-title" data-testid="ov-exams">
      <h2 id="ov-exams-title" class="text-title-md font-bold mb-3">{{ pt('stuExamsTitle') }}</h2>
      <ul class="grid grid-cols-2 gap-3 mb-3">
        <li>
          <router-link :to="{ path: '/platform/my-exams', query: { tab: 'open' } }" class="card-filled block p-4 no-underline h-full" data-testid="ov-open">
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stuOpenNow') }}</span>
            <b class="block text-title-lg"><span dir="ltr" class="inline-block" data-testid="ov-open-n">{{ ov.exams.open }}</span></b>
          </router-link>
        </li>
        <li>
          <router-link :to="{ path: '/platform/my-exams', query: { tab: 'upcoming' } }" class="card-filled block p-4 no-underline h-full" data-testid="ov-upcoming">
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('stuUpcoming') }}</span>
            <b class="block text-title-lg"><span dir="ltr" class="inline-block" data-testid="ov-upcoming-n">{{ ov.exams.upcoming }}</span></b>
          </router-link>
        </li>
      </ul>
      <div v-if="ov.exams.closing_soon.length" data-testid="ov-closing-soon">
        <h3 class="text-title-sm font-bold mb-2">{{ pt('stuClosingSoon') }}</h3>
        <ul class="space-y-2">
          <li v-for="e in ov.exams.closing_soon" :key="e.id">
            <router-link :to="`/platform/assessments/${e.id}`" class="card-filled flex items-center gap-3 p-4 no-underline" :data-testid="`ov-closing-${e.id}`">
              <ClockIcon class="w-6 h-6 shrink-0" style="color: rgb(var(--azl-amber-text, var(--md-primary)))" aria-hidden="true" />
              <span class="flex-1 min-w-0">
                <span class="block font-bold break-words" dir="auto">{{ e.title }}</span>
                <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><bdi>{{ e.subject_name }}</bdi> · {{ pt('stuClosesAt') }}: {{ fmt(e.closes_at) }}</span>
              </span>
            </router-link>
          </li>
        </ul>
      </div>
    </section>

    <section v-if="ov.awaiting_grading > 0" aria-labelledby="ov-awaiting-title">
      <h2 id="ov-awaiting-title" class="sr-only">{{ pt('stuAwaitingTitle') }}</h2>
      <!-- the unfiltered list: an exam with a result waiting can still be open (attempts left) or running, so it is not always under "Done" -->
      <router-link to="/platform/my-exams" class="card-filled flex items-center justify-between gap-3 p-4 no-underline" data-testid="ov-awaiting">
        <span class="font-bold">{{ pt('stuAwaitingTitle') }}</span>
        <span class="px-3 rounded-full text-sm font-bold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))"><span dir="ltr" class="inline-block" data-testid="ov-awaiting-n">{{ ov.awaiting_grading }}</span></span>
      </router-link>
    </section>

    <section v-if="ov.recent_results.length" aria-labelledby="ov-results-title" data-testid="ov-results">
      <div class="flex items-center gap-3 mb-3">
        <h2 id="ov-results-title" class="text-title-md font-bold flex-1">{{ pt('stuResultsTitle') }}</h2>
        <router-link to="/platform/my-exams" class="btn-text" data-testid="ov-results-all">{{ pt('stuAllExams') }}</router-link>
      </div>
      <ul class="space-y-2">
        <li v-for="r in ov.recent_results" :key="r.attempt_id">
          <router-link :to="`/platform/attempts/${r.attempt_id}`" class="card-filled flex flex-wrap items-center gap-x-3 gap-y-1 p-4 no-underline" :data-testid="`ov-result-${r.attempt_id}`">
            <span class="flex-1 min-w-0">
              <span class="block font-bold break-words" dir="auto">{{ r.title }}</span>
              <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(r.submitted_at) }}</span>
            </span>
            <span v-if="!r.released || r.score === null" class="text-body-sm font-semibold" data-testid="ov-result-withheld">{{ pt('tkResultAfterClose') }}</span>
            <span v-else class="font-bold">
              <span dir="ltr" class="inline-block" data-testid="ov-result-score">{{ fmtScore(r.score) }} / {{ fmtScore(r.total) }}</span>
              <span v-if="(r.pending ?? 0) > 0" class="ms-2 text-body-sm font-normal" data-testid="ov-result-pending">{{ pt('pendingGrading') }}</span>
              <span v-if="r.passed !== null" class="ms-2 text-body-sm px-2 py-0.5 rounded-full" :style="r.passed ? 'background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))' : 'background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))'">{{ r.passed ? pt('tkPassed') : pt('tkFailed') }}</span>
            </span>
          </router-link>
        </li>
      </ul>
    </section>

    <section v-if="ov.courses.length" aria-labelledby="ov-courses-title" data-testid="ov-courses">
      <h2 id="ov-courses-title" class="text-title-md font-bold mb-3">{{ pt('continueLearning') }}</h2>
      <ul class="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <li v-for="c in ov.courses" :key="c.course_id">
          <router-link :to="`/platform/courses/${c.course_id}`" class="card-filled block p-4 no-underline h-full" :data-testid="`ov-course-${c.course_id}`">
            <span class="block font-bold break-words" dir="auto">{{ c.title }}</span>
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><bdi>{{ c.subject_name }}</bdi> · <span dir="ltr" class="inline-block">{{ pct(c.percent) }}%</span></span>
            <span class="block h-1.5 rounded-full overflow-hidden mt-2" style="background-color: rgb(var(--md-surface-container-high))" aria-hidden="true">
              <span class="block h-full" :style="{ width: pct(c.percent) + '%', backgroundColor: 'rgb(var(--md-primary))' }"></span>
            </span>
          </router-link>
        </li>
      </ul>
    </section>

    <section v-if="ov.next_live" aria-labelledby="ov-live-title">
      <h2 id="ov-live-title" class="text-title-md font-bold mb-3">{{ pt('stuNextLive') }}</h2>
      <router-link :to="`/platform/subjects/${ov.next_live.subject_id}`" class="card-filled flex items-center gap-3 p-4 no-underline" data-testid="ov-next-live">
        <VideoCameraIcon class="w-6 h-6 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
        <span class="flex-1 min-w-0">
          <span class="block font-bold break-words" dir="auto">{{ ov.next_live.title }}</span>
          <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
            <bdi>{{ ov.next_live.subject_name }}</bdi> · {{ fmt(ov.next_live.starts_at) }} · <span dir="ltr" class="inline-block">{{ ov.next_live.duration_min }}</span> {{ pt('minutesShort') }}
          </span>
        </span>
      </router-link>
    </section>

    <section v-if="empty" class="card-filled p-5 space-y-3" aria-labelledby="ov-empty-title" data-testid="ov-empty">
      <h2 id="ov-empty-title" class="text-title-md font-bold">{{ pt('stuEmptyTitle') }}</h2>
      <p class="text-body-md">{{ pt('stuEmptyBody') }}</p>
      <div class="flex flex-wrap gap-2">
        <router-link to="/platform/my-subjects" class="btn-tonal" data-testid="ov-empty-subjects">{{ pt('mysubjTitle') }}</router-link>
        <router-link to="/platform/my-exams" class="btn-outlined" data-testid="ov-empty-exams">{{ pt('stexTitle') }}</router-link>
      </div>
    </section>

    <section v-if="feedHasItems && latest" aria-labelledby="ov-feed-title" data-testid="ov-feed">
      <h2 id="ov-feed-title" class="text-title-md font-bold mb-3">{{ pt('latestContent') }}</h2>
      <ContentLists :bundle="latest" />
    </section>
  </div>
  <PageError v-else-if="settled && failed" :message="pt('stuOverviewFailed')" back-to="/platform/my-subjects" />
  <PageLoading v-else />
</template>
