<script setup lang="ts">
import { computed, onMounted, ref, type Component } from 'vue'
import {
  AcademicCapIcon, BookOpenIcon, CheckCircleIcon, ClipboardDocumentCheckIcon, ClipboardDocumentListIcon, ClockIcon, DocumentTextIcon,
  PencilSquareIcon, RectangleStackIcon, VideoCameraIcon,
} from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useTeacherStats } from '@/lib/teacherStats'
import { getTeacher } from '@/api/platformLearning'
import {
  checklistComplete, draftsCount, examsCount, onboardingSteps, publishedCount, quickBlock, todoItems, type QuickKind, type StepId, type TodoId,
} from '@/utils/teacherOnboarding'

// The teacher's home: the headline numbers, what waits for them (each row links to the screen that resolves it), quick
// actions gated on having an approved subject (and, for exams, on the admin's switch) and a first-run checklist.
const pt = usePt()
const auth = useAuthStore()
const { stats, failed, refresh } = useTeacherStats()
/** null = not read (yet, or the profile request failed): the checklist then leaves the bio step out */
const hasBio = ref<boolean | null>(null)

onMounted(() => {
  refresh()
  const id = auth.profile?.id
  if (id) getTeacher(id).then((t) => { hasBio.value = !!t.bio?.trim() }).catch(() => { hasBio.value = null })
})

const noSubject = computed(() => !!stats.value && (stats.value.teaching?.approved ?? 0) === 0)
const requestWaiting = computed(() => (stats.value?.teaching?.pending ?? 0) > 0)

interface Kpi { label: PlatformKey; value: number | null; icon: Component; testid: string; sub?: { n: number; label: PlatformKey } }
const kpis = computed<Kpi[]>(() => {
  const s = stats.value
  return [
    { label: 'tchKpiSubjects', value: s ? s.teaching?.approved ?? 0 : null, icon: BookOpenIcon, testid: 'kpi-subjects' },
    { label: 'tchKpiRequests', value: s ? s.teaching?.pending ?? 0 : null, icon: ClockIcon, testid: 'kpi-requests' },
    { label: 'tchKpiDrafts', value: s ? draftsCount(s) : null, icon: PencilSquareIcon, testid: 'kpi-drafts' },
    { label: 'tchKpiPublished', value: s ? publishedCount(s) : null, icon: RectangleStackIcon, testid: 'kpi-published' },
    {
      label: 'tchKpiExams', value: s ? examsCount(s) : null, icon: ClipboardDocumentListIcon, testid: 'kpi-exams',
      sub: s ? { n: s.assessments?.attempts_submitted ?? 0, label: 'tchSubmissions' } : undefined,
    },
    { label: 'tchKpiGrading', value: s ? s.pending_grading?.answers ?? 0 : null, icon: ClipboardDocumentCheckIcon, testid: 'kpi-grading' },
  ]
})

const TODO_LABEL: Record<TodoId, PlatformKey> = {
  grading: 'tchTodoGrading', pending: 'tchTodoPending', rejected: 'tchTodoRejected', unread: 'tchTodoUnread', drafts: 'tchTodoDrafts',
}
const todo = computed(() => (stats.value ? todoItems(stats.value) : []))

interface Quick { id: QuickKind; to: string; label: PlatformKey; desc?: PlatformKey; icon: Component; testid: string }
const QUICK: Quick[] = [
  { id: 'subjects', to: '/platform/teaching', label: 'tchNavSubjects', desc: 'myTeachingDesc', icon: BookOpenIcon, testid: 'quick-subjects' },
  { id: 'content', to: '/platform/my-content', label: 'myContent', desc: 'myContentDesc', icon: RectangleStackIcon, testid: 'quick-content' },
  { id: 'post', to: '/platform/posts/new', label: 'newPost', icon: DocumentTextIcon, testid: 'quick-post' },
  { id: 'course', to: '/platform/courses/new', label: 'newCourse', icon: AcademicCapIcon, testid: 'quick-course' },
  { id: 'live', to: '/platform/live/new', label: 'newLive', icon: VideoCameraIcon, testid: 'quick-live' },
  { id: 'exam', to: '/platform/assessments/new', label: 'exNew', icon: ClipboardDocumentListIcon, testid: 'quick-exam' },
]
const quick = computed(() => QUICK.map((q) => ({ ...q, block: quickBlock(stats.value, q.id) })))
const BLOCK_REASON = { no_subject: 'tchQuickNoSubject', exams_off: 'examsOffReason' } as const

const STEP_LABEL: Record<StepId, PlatformKey> = { bio: 'tchStepBio', subject: 'tchStepSubject', publish: 'tchStepPublish' }
const steps = computed(() => onboardingSteps(stats.value, hasBio.value))
const showChecklist = computed(() => steps.value.length > 0 && !checklistComplete(steps.value))
</script>

<template>
  <div class="max-w-5xl mx-auto pb-8" data-testid="teacher-overview">
    <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('tchOverview') }}</h1>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">{{ pt('welcome') }} <bdi>{{ auth.profile?.full_name }}</bdi></p>

    <p v-if="failed && !stats" class="text-body-md mb-4" role="alert" style="color: rgb(var(--md-error))" data-testid="stats-failed">{{ pt('tchStatsFailed') }}</p>

    <!-- no approved subject yet: nothing can be published, so say so first and offer the one thing to do -->
    <section v-if="noSubject" class="card-elevated p-5 mb-6 flex flex-col sm:flex-row sm:items-center gap-4" aria-labelledby="tch-nosubject-title" data-testid="no-subject-cta">
      <div class="flex-1 min-w-0">
        <h2 id="tch-nosubject-title" class="text-title-md font-bold mb-1">{{ pt('tchNoSubjectTitle') }}</h2>
        <p class="text-body-md">{{ requestWaiting ? pt('tchNoSubjectWaiting') : pt('tchNoSubjectBody') }}</p>
      </div>
      <router-link to="/platform/teaching" class="btn-filled shrink-0" data-testid="cta-request-subject">{{ pt('tchRequestSubject') }}</router-link>
    </section>

    <section class="mb-6" :aria-label="pt('tchKpiTitle')">
      <ul class="grid grid-cols-2 lg:grid-cols-3 gap-3" data-testid="kpis">
        <li v-for="k in kpis" :key="k.testid" class="card-filled p-4 flex items-center gap-3">
          <span class="w-12 h-12 shrink-0 rounded-2xl flex items-center justify-center" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" aria-hidden="true">
            <component :is="k.icon" class="w-6 h-6" />
          </span>
          <span class="min-w-0">
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(k.label) }}</span>
            <b class="block text-title-lg"><span dir="ltr" class="inline-block" :data-testid="k.testid">{{ k.value ?? '—' }}</span></b>
            <span v-if="k.sub" class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block" data-testid="kpi-submissions">{{ k.sub.n }}</span> {{ pt(k.sub.label) }}</span>
          </span>
        </li>
      </ul>
    </section>

    <section class="mb-6">
      <h2 class="text-title-md font-bold mb-3">{{ pt('tchTodoTitle') }}</h2>
      <p v-if="stats && !todo.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="todo-empty">{{ pt('tchTodoEmpty') }}</p>
      <ul class="space-y-2" data-testid="todo-list">
        <li v-for="t in todo" :key="t.id">
          <router-link :to="t.to" :data-testid="'todo-' + t.id" class="card-filled flex items-center justify-between gap-3 p-4 no-underline">
            <span class="font-bold">{{ pt(TODO_LABEL[t.id]) }}</span>
            <span class="px-3 rounded-full text-sm font-bold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))"><span dir="ltr" class="inline-block" :data-testid="'todo-' + t.id + '-n'">{{ t.n }}</span></span>
          </router-link>
        </li>
      </ul>
    </section>

    <section v-if="showChecklist" class="mb-6" data-testid="onboarding-checklist">
      <h2 class="text-title-md font-bold mb-3">{{ pt('tchChecklistTitle') }}</h2>
      <ol class="space-y-2">
        <li v-for="s in steps" :key="s.id">
          <router-link :to="s.to" :data-testid="'check-' + s.id" :data-done="s.done ? 'true' : 'false'" class="card-filled flex items-center gap-3 p-4 no-underline">
            <CheckCircleIcon v-if="s.done" class="w-6 h-6 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
            <span v-else class="w-6 h-6 shrink-0 rounded-full border-2" style="border-color: rgb(var(--md-outline))" aria-hidden="true"></span>
            <span class="flex-1 font-bold">{{ pt(STEP_LABEL[s.id]) }}</span>
            <span class="sr-only">{{ s.done ? pt('tchStepDone') : pt('tchStepTodo') }}</span>
          </router-link>
        </li>
      </ol>
    </section>

    <section class="mb-2">
      <h2 class="text-title-md font-bold mb-3">{{ pt('tchQuickTitle') }}</h2>
      <ul class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3" data-testid="quick-actions">
        <li v-for="q in quick" :key="q.id">
          <router-link v-if="!q.block" :to="q.to" :data-testid="q.testid" class="card-filled flex items-start gap-3 p-4 no-underline h-full">
            <component :is="q.icon" class="w-6 h-6 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
            <span class="min-w-0">
              <span class="block font-bold">{{ pt(q.label) }}</span>
              <span v-if="q.desc" class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(q.desc) }}</span>
            </span>
          </router-link>
          <!-- not available yet: a plain card that says why (a dead link would only lead to an error) -->
          <div v-else class="card-filled flex items-start gap-3 p-4 h-full" :data-testid="q.testid" data-disabled="true">
            <component :is="q.icon" class="w-6 h-6 shrink-0" style="color: rgb(var(--md-on-surface-variant))" aria-hidden="true" />
            <span class="min-w-0">
              <span class="block font-bold">{{ pt(q.label) }}</span>
              <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))" :data-testid="q.testid + '-reason'">{{ pt(BLOCK_REASON[q.block]) }}</span>
            </span>
          </div>
        </li>
      </ul>
    </section>
  </div>
</template>
