<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myStats, type StudentStats, type SubjectStats } from '@/api/platformStudent'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import { fillTemplate } from '@/utils/notificationText'
import { barCss, barLabel, fmtPercent, orderSubjects, statsEmpty, subjectHasResults, trendDirection } from '@/utils/progress'

// My progress: the student's own results by subject. The server counts only submitted attempts whose result is released and
// fully graded, takes the best attempt of each exam for the averages, and sends nothing about anybody else - there is no
// ranking and no class average here, and the page says so.
const pt = usePt()
const i18n = useI18nStore()
const stats = ref<StudentStats | null>(null)
const error = ref('')

async function load() {
  error.value = ''
  try {
    stats.value = await myStats()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(load)

const subjects = computed<SubjectStats[]>(() => orderSubjects(stats.value?.subjects ?? []))
const empty = computed(() => statsEmpty(stats.value))
const shortDate = (ms: number) => new Date(ms).toLocaleDateString(i18n.locale === 'ar' ? 'ar' : undefined, { day: 'numeric', month: 'short' })
const pct = (p: number | null | undefined) => fmtPercent(p)
const direction = (s: SubjectStats) => trendDirection(s.trend)
const barsLabel = (s: SubjectStats) => fillTemplate(pt('prgBarsLabel'), { subject: s.subject_name })
</script>

<template>
  <div v-if="stats" class="max-w-3xl mx-auto pb-8 space-y-5" data-testid="progress-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight" data-testid="progress-title">{{ pt('prgTitle') }}</h1>
      <p class="text-body-md mt-2" data-testid="progress-private">{{ pt('prgPrivate') }}</p>
      <p class="text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt('prgHowCounted') }}</p>
    </div>

    <p v-if="empty" class="card-filled p-4 text-body-md" data-testid="progress-empty">
      {{ pt('prgEmpty') }} <router-link to="/platform/my-exams" class="underline font-semibold" data-testid="progress-go-exams">{{ pt('stexTitle') }}</router-link>
    </p>

    <section aria-labelledby="prg-overall" class="card-elevated p-5" data-testid="progress-overall">
      <h2 id="prg-overall" class="text-title-lg font-bold mb-3">{{ pt('prgOverall') }}</h2>
      <dl class="grid grid-cols-3 gap-2 text-center">
        <div class="card-filled p-3" data-testid="progress-overall-attempts"><dt class="text-body-sm">{{ pt('prgAttempts') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ stats.overall.attempts }}</dd></div>
        <div class="card-filled p-3" data-testid="progress-overall-exams"><dt class="text-body-sm">{{ pt('prgExams') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ stats.overall.exams }}</dd></div>
        <div class="card-filled p-3" data-testid="progress-overall-average"><dt class="text-body-sm">{{ pt('prgAverage') }}</dt><dd class="text-title-lg font-bold" dir="ltr">{{ pct(stats.overall.average_percent) !== null ? `${pct(stats.overall.average_percent)}%` : '—' }}</dd></div>
      </dl>
    </section>

    <h2 v-if="subjects.length" class="text-title-lg font-bold" data-testid="progress-by-subject">{{ pt('prgBySubject') }}</h2>
    <section
      v-for="s in subjects" :key="s.subject_id" :aria-labelledby="`prg-s-${s.subject_id}`" class="card-elevated p-5 space-y-4" :data-testid="`progress-subject-${s.subject_id}`"
    >
      <h3 :id="`prg-s-${s.subject_id}`" class="text-title-md font-bold break-words" dir="auto">{{ s.subject_name }}</h3>
      <dl class="grid grid-cols-3 gap-2 text-center">
        <div class="card-filled p-3"><dt class="text-body-sm">{{ pt('prgAttempts') }}</dt><dd class="text-title-md font-bold" dir="ltr">{{ s.attempts }}</dd></div>
        <div class="card-filled p-3"><dt class="text-body-sm">{{ pt('prgExams') }}</dt><dd class="text-title-md font-bold" dir="ltr">{{ s.exams }}</dd></div>
        <div class="card-filled p-3" :data-testid="`progress-average-${s.subject_id}`"><dt class="text-body-sm">{{ pt('prgAverage') }}</dt><dd class="text-title-md font-bold" dir="ltr">{{ pct(s.average_percent) !== null ? `${pct(s.average_percent)}%` : '—' }}</dd></div>
      </dl>

      <p v-if="!subjectHasResults(s)" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('prgNoResults') }}</p>

      <!-- the latest attempts, oldest to newest: every bar shows its number and is named with the exam and the percent -->
      <div v-if="s.trend.length" :data-testid="`progress-trend-${s.subject_id}`">
        <h4 class="text-label-lg font-bold mb-2">{{ pt('prgTrend') }}</h4>
        <ol class="flex gap-1 sm:gap-2 list-none p-0 m-0" :aria-label="barsLabel(s)">
          <li v-for="p in s.trend" :key="p.attempt_id" class="flex-1 min-w-0 flex flex-col items-center">
            <span class="text-xs font-bold tabular-nums" dir="ltr" data-testid="progress-bar-value">{{ pct(p.percent) }}%</span>
            <span class="w-full h-28 flex items-end justify-center">
              <span
                role="img" :aria-label="barLabel(p)" class="block w-full max-w-[3rem] rounded-t-lg" :style="{ height: barCss(p.percent), backgroundColor: 'rgb(var(--md-primary))', minHeight: '2px' }"
                :data-testid="`progress-bar-${p.attempt_id}`"
              ></span>
            </span>
            <span class="w-full border-t pt-1 text-[11px] text-center truncate" style="border-color: rgb(var(--md-outline)); color: rgb(var(--md-on-surface-variant))" dir="auto" aria-hidden="true">{{ shortDate(p.submitted_at) }}</span>
          </li>
        </ol>
        <p v-if="direction(s) !== 'none'" class="text-body-sm mt-2 font-semibold" :data-testid="`progress-direction-${s.subject_id}`">{{ pt(`prgTrend_${direction(s)}` as PlatformKey) }}</p>
      </div>

      <div v-if="subjectHasResults(s)" :data-testid="`progress-weak-${s.subject_id}`">
        <h4 class="text-label-lg font-bold">{{ pt('prgWeakest') }}</h4>
        <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">{{ pt('prgWeakHint') }}</p>
        <ul v-if="s.weakest.length" class="space-y-1">
          <li v-for="w in s.weakest" :key="w.chapter" class="card-filled p-3 flex flex-wrap items-center gap-x-3 gap-y-1">
            <span class="font-semibold flex-1 min-w-0 break-words" dir="auto">{{ w.chapter }}</span>
            <span class="font-bold" dir="ltr">{{ pct(w.percent) }}%</span>
            <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ w.answered }}</span> {{ pt('prgAnswered') }}</span>
          </li>
        </ul>
        <p v-else class="text-body-sm" data-testid="progress-no-weak">{{ pt('prgNoWeak') }}</p>
      </div>
    </section>
  </div>
  <PageError v-else-if="error" :message="error" back-to="/platform/student" />
  <PageLoading v-else />
</template>
