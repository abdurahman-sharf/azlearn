<script setup lang="ts">
import { ref, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useRouteParam } from '@/composables/useRouteParam'
import ReportButton from '@/components/platform/ReportButton.vue'
import ReviewsPanel from '@/components/platform/ReviewsPanel.vue'
import ContentLists from '@/components/platform/ContentLists.vue'
import { teacherContent, type Bundle } from '@/api/platformContent'
import { getTeacher, follow, unfollow, type TeacherPage } from '@/api/platformLearning'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'

const pt = usePt()
const auth = useAuthStore()
const id = useRouteParam('id')

const teacher = ref<TeacherPage | null>(null)
const content = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')

// Answers to the latest load only: a slow answer for the teacher we just left must not land on the one we are opening.
let seq = 0
async function load() {
  const mine = ++seq
  const teacherId = id.value
  try {
    const t = await getTeacher(teacherId)
    const c = await teacherContent(teacherId)
    if (mine !== seq) return
    teacher.value = t
    content.value = c
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  }
}

async function toggleFollow() {
  if (!teacher.value) return
  try {
    await (teacher.value.following ? unfollow(id.value) : follow(id.value))
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

// Moving to another teacher (a link, a notification, back/forward) re-uses this page: forget the old one and load the new.
watch(id, () => {
  seq++
  teacher.value = null
  content.value = { posts: [], courses: [], live: [] }
  error.value = ''
  void load()
}, { immediate: true })
</script>

<template>
  <div v-if="teacher" class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform/teachers" class="text-body-sm underline">{{ pt('browseTeachers') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3" dir="auto">{{ teacher.full_name }}</h1>
    <p class="text-body-sm mb-3" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ teacher.followers }}</span> {{ pt('followers') }}</p>
    <p v-if="teacher.bio" class="text-body-lg mb-4 whitespace-pre-line" dir="auto" data-testid="teacher-bio">{{ teacher.bio }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <button v-if="auth.role === 'student'" :class="teacher.following ? 'btn-outlined' : 'btn-filled'" class="mb-6" data-testid="teacher-follow" @click="toggleFollow">
      {{ teacher.following ? pt('unfollow') : pt('follow') }}
    </button>

    <ContentLists :bundle="content" class="mb-6" />

    <div v-if="auth.profile?.id !== teacher.id" class="mb-4"><ReportButton target-type="teacher" :target-id="teacher.id" /></div>

    <ReviewsPanel :target-id="id" target-type="teacher" class="mb-6" />

    <h2 class="text-title-md font-bold mb-3">{{ pt('teachesSubjects') }}</h2>
    <p v-if="!teacher.subjects.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-2">
      <li v-for="s in teacher.subjects" :key="s.subject_id">
        <router-link :to="`/platform/subjects/${s.subject_id}`" class="card-filled block p-3">
          <div class="font-bold">{{ s.subject_name }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ s.institution_name }}</div>
        </router-link>
      </li>
    </ul>
  </div>
  <PageError v-else-if="error" :message="error" back-to="/platform/teachers" />
  <PageLoading v-else />
</template>
