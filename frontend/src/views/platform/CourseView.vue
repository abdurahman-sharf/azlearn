<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import ReviewsPanel from '@/components/platform/ReviewsPanel.vue'
import { courseProgress, setLessonDone } from '@/api/platformEngage'
import { getSubject } from '@/api/platformLearning'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getCourse, updateCourse, deleteCourse, type CourseDetail, type Lesson } from '@/api/platformContent'
import PageError from '@/components/platform/PageError.vue'

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string

const course = ref<CourseDetail | null>(null)
const current = ref<Lesson | null>(null)
const error = ref('')
const done = ref<Set<string>>(new Set())
const enrolled = ref(false)
const percent = computed(() => (course.value?.lessons.length ? Math.round((done.value.size / course.value.lessons.length) * 100) : 0))

const groups = computed(() => {
  const out: { section: string | null; lessons: Lesson[] }[] = []
  for (const l of course.value?.lessons ?? []) {
    const last = out[out.length - 1]
    if (last && last.section === l.section) last.lessons.push(l)
    else out.push({ section: l.section, lessons: [l] })
  }
  return out
})

async function load() {
  try {
    course.value = await getCourse(id)
    if (auth.role === 'student') {
      enrolled.value = (await getSubject(course.value.subject_id)).enrolled
      if (enrolled.value) done.value = new Set((await courseProgress(id)).completed)
    }
    current.value = current.value
      ? course.value.lessons.find(l => l.id === current.value!.id) ?? course.value.lessons[0] ?? null
      : course.value.lessons[0] ?? null
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function toggleDone(l: Lesson) {
  const next = !done.value.has(l.id)
  try {
    await setLessonDone(l.id, next)
    const s = new Set(done.value)
    if (next) s.add(l.id)
    else s.delete(l.id)
    done.value = s
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function unpublish() {
  try { await updateCourse(id, { status: 'draft' }); await load() } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function remove() {
  if (!window.confirm(pt('confirmDelete'))) return
  try { await deleteCourse(id); router.replace('/platform') } catch (e) { error.value = platformErrorMessage(pt, e) }
}
onMounted(load)
</script>

<template>
  <div v-if="course" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${course.subject_id}`" class="text-body-sm underline">{{ course.subject_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words" dir="auto">{{ course.title }}</h1>
    <p class="text-body-sm mb-2" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('by') }} <router-link :to="`/platform/teachers/${course.teacher_id}`" class="underline">{{ course.teacher_name }}</router-link>
      · {{ course.lesson_count }} {{ pt('lessonsCount') }}<template v-if="course.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <p v-if="course.description" class="text-body-lg mb-4 whitespace-pre-line" dir="auto">{{ course.description }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <!-- Player: embeds only server-computed provider URLs; anything else is an external link. -->
    <div v-if="current" class="mb-4">
      <div class="font-bold mb-2 break-words">{{ current.title }}</div>
      <div v-if="current.embed_url && (current.video_kind === 'youtube' || current.video_kind === 'vimeo')" class="aspect-video w-full overflow-hidden rounded-2xl">
        <iframe
          :key="current.id"
          :src="current.embed_url"
          class="w-full h-full"
          allow="fullscreen; picture-in-picture"
          allowfullscreen
          referrerpolicy="strict-origin-when-cross-origin"
          sandbox="allow-scripts allow-same-origin allow-presentation"
          :title="current.title"
        ></iframe>
      </div>
      <video v-else-if="current.video_kind === 'file' && current.embed_url" :key="current.id" :src="current.embed_url" controls preload="metadata" class="w-full rounded-2xl"></video>
      <div v-else class="card-filled p-4">
        <p class="text-body-sm mb-2">{{ pt('externalVideo') }}</p>
        <a :href="current.video_url" target="_blank" rel="noopener noreferrer" class="btn-tonal">{{ pt('openLink') }}</a>
      </div>
      <p v-if="current.description" class="text-body-lg mt-3 whitespace-pre-line" dir="auto">{{ current.description }}</p>
      <button v-if="enrolled" class="mt-3" :class="done.has(current.id) ? 'btn-outlined' : 'btn-filled'" @click="toggleDone(current)">
        {{ done.has(current.id) ? pt('markUndone') : pt('markDone') }}
      </button>
    </div>

    <div v-if="auth.role === 'student'" class="mb-4">
      <p v-if="!enrolled" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('enrollToTrack') }}</p>
      <template v-else>
        <div class="flex justify-between text-body-sm mb-1"><span>{{ pt('progress') }}</span><span data-testid="percent">{{ percent }}%</span></div>
        <div class="h-2 rounded-full overflow-hidden" style="background-color: rgb(var(--md-surface-container-high))" role="progressbar" :aria-label="pt('progress')" :aria-valuenow="percent" aria-valuemin="0" aria-valuemax="100">
          <div class="h-full" :style="{ width: percent + '%', backgroundColor: 'rgb(var(--md-primary))' }"></div>
        </div>
      </template>
    </div>

    <div v-for="(g, gi) in groups" :key="gi" class="mb-3">
      <div v-if="g.section" class="font-semibold mb-1" style="color: rgb(var(--md-on-surface-variant))">{{ g.section }}</div>
      <ul class="space-y-1">
        <li v-for="l in g.lessons" :key="l.id">
          <button class="w-full text-start card-filled p-3" :class="{ 'ring-2': current?.id === l.id }" @click="current = l">{{ l.position }}. {{ l.title }}<span v-if="done.has(l.id)" class="ms-2" style="color: rgb(var(--md-primary))" :aria-label="pt('completedLesson')">✓</span></button>
        </li>
      </ul>
    </div>

    <ReviewsPanel :target-id="id" target-type="course" class="mt-6" />

    <div class="flex flex-wrap gap-2 mt-4">
      <ReportButton v-if="auth.profile?.id !== course.teacher_id" target-type="course" :target-id="course.id" />
      <router-link v-if="auth.profile?.id === course.teacher_id" :to="`/platform/courses/${course.id}/edit`" class="btn-outlined">{{ pt('edit') }}</router-link>
      <template v-if="auth.role === 'admin'">
        <button v-if="course.status === 'published'" class="btn-outlined" @click="unpublish">{{ pt('unpublish') }}</button>
        <button class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </template>
    </div>
  </div>
  <PageError v-else-if="error" :message="error" />
</template>
