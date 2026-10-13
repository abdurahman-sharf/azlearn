<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ArrowRightIcon, ArrowTopRightOnSquareIcon, CheckCircleIcon, LockClosedIcon } from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useRouteParam } from '@/composables/useRouteParam'
import { useEnrollAction } from '@/composables/useEnrollAction'
import ReviewsPanel from '@/components/platform/ReviewsPanel.vue'
import { courseProgress, setLessonDone } from '@/api/platformEngage'
import { getSubject } from '@/api/platformLearning'
import ReportButton from '@/components/platform/ReportButton.vue'
import BookmarkButton from '@/components/platform/BookmarkButton.vue'
import { getCourse, updateCourse, deleteCourse, type CourseDetail } from '@/api/platformContent'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import {
  allDone, doneCount, embedSrc, externalHref, fileSrc, groupBySection, lessonFromQuery, nextLessonId, percentDone, pickInitialLesson, playerKind,
} from '@/utils/lessonPick'

// The course page. A student who is enrolled opens on the lesson named in the address (?lesson=), else the first lesson not
// completed yet, can mark lessons done, sees how far they are and gets a "next lesson" button and a banner when the course
// is finished. A student who is NOT enrolled sees the lessons but not their videos (the server withholds the links: `locked`)
// and can enrol from the lock notice. Teachers and admins keep the plain viewer (no progress). Only the server's embed
// address is ever put into an iframe; every other link is an external link.
const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const id = useRouteParam('id')
const enr = useEnrollAction()

const course = ref<CourseDetail | null>(null)
const currentId = ref<string | null>(null)
const error = ref('')
const done = ref<Set<string>>(new Set())
const enrolled = ref(false)
const videoFailed = ref(false)
const announce = ref('')
const deleting = ref(false)
const deleteBusy = ref(false)
const deleteError = ref('')

const isStudent = computed(() => auth.role === 'student')
/** progress is tracked only for an enrolled student */
const tracking = computed(() => isStudent.value && enrolled.value)
const lessons = computed(() => course.value?.lessons ?? [])
const groups = computed(() => groupBySection(lessons.value))
const current = computed(() => lessons.value.find((l) => l.id === currentId.value) ?? null)
const currentIndex = computed(() => lessons.value.findIndex((l) => l.id === currentId.value))
const nextId = computed(() => nextLessonId(lessons.value, currentId.value))
const doneN = computed(() => doneCount(lessons.value, done.value))
const percent = computed(() => percentDone(lessons.value, done.value))
const finished = computed(() => tracking.value && allDone(lessons.value, done.value))
const kind = computed(() => (current.value ? playerKind(current.value, videoFailed.value) : null))

// Answers to the latest load only: a slow answer for the course we just left must not land on the one we are opening.
let seq = 0
async function load(keepLesson = false) {
  const mine = ++seq
  error.value = ''
  try {
    const c = await getCourse(id.value)
    if (mine !== seq) return
    let isEnrolled = false
    let completed = new Set<string>()
    if (isStudent.value) {
      // the course says whether I am enrolled; an older server that does not is asked through the subject page
      isEnrolled = c.enrolled ?? (await getSubject(c.subject_id)).enrolled
      if (mine !== seq) return
      if (isEnrolled) completed = new Set((await courseProgress(id.value)).completed)
      if (mine !== seq) return
    }
    course.value = c
    enrolled.value = isEnrolled
    done.value = completed
    const keep = keepLesson && currentId.value && c.lessons.some((l) => l.id === currentId.value) ? currentId.value : null
    currentId.value = pickInitialLesson(c.lessons, completed, keep ?? lessonFromQuery(route.query), isStudent.value && isEnrolled)
    videoFailed.value = false
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  }
}

// Moving to another course (a link, a notification, back/forward) re-uses this page: forget the old one and load the new.
watch(id, () => {
  seq++
  course.value = null
  currentId.value = null
  done.value = new Set()
  enrolled.value = false
  error.value = ''
  announce.value = ''
  deleting.value = false
  void load()
}, { immediate: true })

const titleEl = ref<HTMLElement | null>(null)
const playerEl = ref<HTMLElement | null>(null)

/**
 * After the lesson changed: the lesson's heading takes the focus, and the player is scrolled into view whenever it is not
 * entirely on screen (the lesson list sits below it on every screen size, so a click far down the list moves it off screen).
 */
async function revealLesson() {
  await nextTick()
  titleEl.value?.focus({ preventScroll: true })
  const player = playerEl.value
  if (typeof window !== 'undefined' && player) {
    const box = player.getBoundingClientRect()
    if (box.top < 0 || box.bottom > window.innerHeight) {
      const calm = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
      player.scrollIntoView({ block: 'start', behavior: calm ? 'auto' : 'smooth' })
    }
  }
}

/** Says `text` in the status region. The region is emptied first, so the same words are announced again. */
async function say(text: string) {
  announce.value = ''
  await nextTick()
  announce.value = text
}

function select(lessonId: string | null, updateUrl: boolean) {
  if (!lessonId || lessonId === currentId.value || !lessons.value.some((l) => l.id === lessonId)) return
  currentId.value = lessonId
  videoFailed.value = false
  if (updateUrl) router.replace({ query: { ...route.query, lesson: lessonId } }).catch(() => { /* a redundant navigation is not an error */ })
  void revealLesson()
}

// The address named another lesson of this course (an overview card, a notification link): follow it.
watch(() => route.query.lesson, () => {
  const wanted = lessonFromQuery(route.query)
  if (course.value && wanted) select(wanted, false)
})

const toggling = ref(false)
async function toggleDone(lessonId: string) {
  if (toggling.value) return
  const next = !done.value.has(lessonId)
  toggling.value = true
  try {
    await setLessonDone(lessonId, next)
    const s = new Set(done.value)
    if (next) s.add(lessonId)
    else s.delete(lessonId)
    done.value = s
    // the lesson and the new count are part of the message: marking the next lesson reads differently from the last one
    const title = lessons.value.find((l) => l.id === lessonId)?.title ?? ''
    void say(`${next ? pt('crsMarkedDone') : pt('crsMarkedUndone')} ${title} (${doneN.value} / ${lessons.value.length})`)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    toggling.value = false
  }
}

async function enrollNow() {
  if (!course.value) return
  const ok = await enr.run(course.value.subject_id, () => load(true))
  // the button that had the focus is gone with the lock notice: the focus moves to the lesson that just unlocked
  if (ok) {
    await revealLesson()
    void say(pt('crsEnrolledNow'))
  }
}

async function unpublish() {
  try { await updateCourse(id.value, { status: 'draft' }); await load(true) } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function remove() {
  if (deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await deleteCourse(id.value)
    deleting.value = false
    await router.replace('/platform')
  } catch (e) {
    deleteError.value = platformErrorMessage(pt, e)
  } finally {
    deleteBusy.value = false
  }
}
</script>

<template>
  <div v-if="course" class="max-w-3xl mx-auto pb-8" data-testid="course-page">
    <router-link :to="`/platform/subjects/${course.subject_id}`" class="text-body-sm underline">{{ course.subject_name }}</router-link>
    <div class="flex flex-wrap items-start gap-3 mt-3">
      <h1 class="flex-1 min-w-[12rem] text-display-sm font-bold tracking-tight break-words" dir="auto">{{ course.title }}</h1>
      <BookmarkButton v-if="isStudent" target-type="course" :target-id="course.id" testid="course-bookmark" />
    </div>
    <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('by') }} <router-link :to="`/platform/teachers/${course.teacher_id}`" class="underline">{{ course.teacher_name }}</router-link>
      · <span dir="ltr" class="inline-block">{{ course.lesson_count }}</span> {{ pt('lessonsCount') }}<template v-if="course.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <p v-if="course.description" class="text-body-lg mb-4 whitespace-pre-line" dir="auto">{{ course.description }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="course-error">{{ error }}</p>
    <p class="sr-only" role="status" data-testid="course-announce">{{ announce }}</p>

    <!-- progress: only for a student who is enrolled -->
    <div v-if="isStudent" class="mb-4">
      <p v-if="!enrolled" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="course-enroll-hint">{{ pt('enrollToTrack') }}</p>
      <template v-else>
        <div class="flex justify-between text-body-sm mb-1">
          <span>{{ pt('progress') }}</span>
          <span><span dir="ltr" class="inline-block" data-testid="course-done-count">{{ doneN }} / {{ lessons.length }}</span> · <span dir="ltr" class="inline-block" data-testid="percent">{{ percent }}%</span></span>
        </div>
        <div
          class="h-2 rounded-full overflow-hidden"
          style="background-color: rgb(var(--md-surface-container-high))"
          role="progressbar"
          :aria-label="pt('progress')"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-valuenow="percent"
          :aria-valuetext="`${doneN} / ${lessons.length}`"
          data-testid="course-progress"
        >
          <div class="h-full" :style="{ width: percent + '%', backgroundColor: 'rgb(var(--md-primary))' }"></div>
        </div>
      </template>
    </div>

    <div v-if="finished" class="card-filled p-4 mb-4 flex flex-wrap items-center gap-3" role="status" data-testid="course-complete" style="border-inline-start: 6px solid rgb(var(--azl-success-text, var(--md-primary)))">
      <CheckCircleIcon class="w-7 h-7 shrink-0" style="color: rgb(var(--azl-success-text, var(--md-primary)))" aria-hidden="true" />
      <span class="flex-1 min-w-[10rem] font-bold">{{ pt('crsCompleted') }}</span>
      <router-link to="/platform/learning" class="btn-tonal" data-testid="course-complete-link">{{ pt('crsAllCourses') }}</router-link>
    </div>

    <!-- Player: embeds only server-computed provider URLs; anything else is an external link. -->
    <div v-if="current" ref="playerEl" class="mb-4 scroll-mt-24" data-testid="lesson-player">
      <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('crsLesson') }} <span dir="ltr" class="inline-block">{{ currentIndex + 1 }} / {{ lessons.length }}</span></p>
      <h2 ref="titleEl" tabindex="-1" class="font-bold text-title-lg mb-2 break-words focus:outline-none" dir="auto" data-testid="lesson-title">{{ current.title }}</h2>

      <div v-if="kind === 'locked'" class="card-filled p-4 space-y-3" data-testid="lesson-locked">
        <p class="flex items-center gap-2 font-semibold"><LockClosedIcon class="w-5 h-5 shrink-0" aria-hidden="true" /> <span>{{ pt('crsLocked') }}</span></p>
        <p class="text-body-sm">{{ isStudent ? pt('crsLockedWhy') : pt('crsLockedOther') }}</p>
        <button v-if="isStudent && !enrolled" type="button" class="btn-filled" :disabled="enr.busy.value" data-testid="lesson-enroll" @click="enrollNow">{{ pt('enroll') }}</button>
        <p v-if="enr.error.value" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ enr.error.value }}</p>
      </div>
      <div v-else-if="kind === 'embed'" class="aspect-video w-full overflow-hidden rounded-2xl">
        <iframe
          :key="current.id"
          :src="embedSrc(current) ?? undefined"
          class="w-full h-full"
          allow="fullscreen; picture-in-picture"
          allowfullscreen
          referrerpolicy="strict-origin-when-cross-origin"
          sandbox="allow-scripts allow-same-origin allow-presentation"
          :title="current.title"
          data-testid="lesson-embed"
        ></iframe>
      </div>
      <video
        v-else-if="kind === 'file'"
        :key="current.id"
        :src="fileSrc(current) ?? undefined"
        controls
        preload="metadata"
        class="w-full rounded-2xl"
        :aria-label="current.title"
        data-testid="lesson-video"
        @error="videoFailed = true"
      ></video>
      <div v-else class="card-filled p-4 space-y-2">
        <p class="text-body-sm">{{ current.video_kind === 'file' ? pt('crsVideoFailed') : pt('externalVideo') }}</p>
        <a
          v-if="externalHref(current)"
          :href="externalHref(current) ?? undefined"
          target="_blank"
          rel="noopener noreferrer"
          class="btn-tonal inline-flex items-center gap-2"
          data-testid="lesson-open-link"
        >{{ pt('openLink') }}<ArrowTopRightOnSquareIcon class="w-4 h-4" aria-hidden="true" /><span class="sr-only">{{ pt('opensInNewTab') }}</span></a>
      </div>

      <p v-if="current.description && kind !== 'locked'" class="text-body-lg mt-3 whitespace-pre-line" dir="auto">{{ current.description }}</p>
      <p v-else-if="current.description" class="text-body-md mt-3 whitespace-pre-line" dir="auto">{{ current.description }}</p>

      <div class="flex flex-wrap items-center gap-2 mt-3">
        <button
          v-if="tracking && kind !== 'locked'"
          type="button"
          :class="done.has(current.id) ? 'btn-outlined' : 'btn-filled'"
          :disabled="toggling"
          data-testid="lesson-complete"
          @click="toggleDone(current.id)"
        >
          <CheckCircleIcon v-if="done.has(current.id)" class="w-5 h-5 inline-block align-middle me-1" aria-hidden="true" />
          {{ done.has(current.id) ? pt('markUndone') : pt('markDone') }}
        </button>
        <button v-if="nextId" type="button" class="btn-tonal inline-flex items-center gap-2" data-testid="course-next" @click="select(nextId, true)">
          {{ pt('crsNextLesson') }}<ArrowRightIcon class="w-4 h-4 rtl:-scale-x-100" aria-hidden="true" />
        </button>
      </div>
    </div>
    <p v-else class="text-body-md mb-4" style="color: rgb(var(--md-on-surface-variant))" data-testid="course-no-lessons">{{ pt('crsNoLessons') }}</p>

    <section v-if="lessons.length" aria-labelledby="course-lessons-title" data-testid="lesson-list">
      <h2 id="course-lessons-title" class="text-title-md font-bold mb-2">{{ pt('crsLessons') }}</h2>
      <div v-for="(g, gi) in groups" :key="gi" class="mb-3">
        <h3 v-if="g.section" class="font-semibold mb-1" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ g.section }}</h3>
        <ul class="space-y-1">
          <li v-for="l in g.lessons" :key="l.id">
            <button
              type="button"
              class="w-full text-start card-filled p-3 flex items-center gap-3"
              :class="current?.id === l.id ? 'ring-2 font-bold' : ''"
              :aria-current="current?.id === l.id ? 'true' : undefined"
              :data-testid="`lesson-item-${l.id}`"
              @click="select(l.id, true)"
            >
              <span class="flex-1 min-w-0 break-words" dir="auto"><span dir="ltr" class="inline-block">{{ l.position }}.</span> {{ l.title }}</span>
              <span v-if="tracking && done.has(l.id)" class="shrink-0 inline-flex items-center gap-1 text-body-sm font-semibold" style="color: rgb(var(--azl-success-text, var(--md-primary)))" :data-testid="`lesson-done-${l.id}`">
                <CheckCircleIcon class="w-5 h-5" aria-hidden="true" />{{ pt('completedLesson') }}
              </span>
              <span v-if="l.locked" class="shrink-0 inline-flex items-center gap-1 text-body-sm font-semibold" :data-testid="`lesson-lock-${l.id}`">
                <LockClosedIcon class="w-5 h-5" aria-hidden="true" />{{ pt('crsLockedShort') }}
              </span>
            </button>
          </li>
        </ul>
      </div>
    </section>

    <ReviewsPanel :target-id="id" target-type="course" class="mt-6" />

    <div class="flex flex-wrap gap-2 mt-4">
      <ReportButton v-if="auth.profile?.id !== course.teacher_id" target-type="course" :target-id="course.id" />
      <router-link v-if="auth.profile?.id === course.teacher_id" :to="`/platform/courses/${course.id}/edit`" class="btn-outlined">{{ pt('edit') }}</router-link>
      <template v-if="auth.role === 'admin'">
        <button v-if="course.status === 'published'" class="btn-outlined" @click="unpublish">{{ pt('unpublish') }}</button>
        <button class="btn-outlined" data-testid="course-admin-delete" @click="deleteError = ''; deleting = true">{{ pt('del') }}</button>
      </template>
    </div>
    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('del')}: ${course.title}`"
      :message="course.status === 'published' ? `${pt('edDeleteCourseMsg')} ${pt('edDeletePublished')}` : pt('edDeleteCourseMsg')"
      :counts="[{ label: pt('lessonsCount'), value: course.lessons.length }]"
      :kept="pt('edDeleteCourseKept')"
      :confirm-name="course.status === 'published' ? course.title : undefined"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="remove"
      @close="deleting = false"
    />
  </div>
  <PageError v-else-if="error" :message="error" />
  <PageLoading v-else />
</template>
