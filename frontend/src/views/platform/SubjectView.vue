<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useRouteParam } from '@/composables/useRouteParam'
import { useStudentStats } from '@/lib/studentStats'
import AssessmentList from '@/components/platform/AssessmentList.vue'
import { subjectAssessments, type AssessmentInfo } from '@/api/platformExams'
import ContentLists from '@/components/platform/ContentLists.vue'
import { subjectContent, type Bundle } from '@/api/platformContent'
import { getSubject, enroll, unenroll, myTeaching, requestTeaching, type SubjectPage, type Teaching } from '@/api/platformLearning'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

const pt = usePt()
const auth = useAuthStore()
const id = useRouteParam('id')
const { refreshAfterChange } = useStudentStats()

const subject = ref<SubjectPage | null>(null)
const content = ref<Bundle>({ posts: [], courses: [], live: [] })
const assessments = ref<AssessmentInfo[]>([])
const mine_ = ref<Teaching | null>(null)
const error = ref('')

const teachingState = computed(() => mine_.value?.status ?? null)

// Answers to the latest load only: a slow answer for the subject we just left must not land on the one we are opening.
let seq = 0
async function load() {
  const mine = ++seq
  error.value = ''
  const subjectId = id.value
  try {
    // independent requests: wait for them together
    const [sub, cont, asm, teaching] = await Promise.all([
      getSubject(subjectId),
      subjectContent(subjectId),
      subjectAssessments(subjectId),
      auth.role === 'teacher' ? myTeaching() : Promise.resolve(null),
    ])
    if (mine !== seq) return
    subject.value = sub
    content.value = cont
    assessments.value = asm
    if (teaching) mine_.value = teaching.find(t => t.subject_id === subjectId) ?? null
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  }
}

async function act(fn: () => Promise<unknown>) {
  error.value = ''
  try {
    await fn()
    await load()
    // enrolling / leaving changes what the student can open (sidebar badges, overview)
    if (auth.role === 'student') void refreshAfterChange()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

// Leaving a subject asks first: its exams and content disappear from the student's lists (results stay).
const leaving = ref(false)
const leaveBusy = ref(false)
const leaveError = ref('')
async function confirmLeave() {
  if (leaveBusy.value) return
  leaveBusy.value = true
  leaveError.value = ''
  try {
    await unenroll(id.value)
    leaving.value = false
    await load()
    void refreshAfterChange()
  } catch (e) {
    leaveError.value = platformErrorMessage(pt, e)
  } finally {
    leaveBusy.value = false
  }
}

// Moving to another subject (a link, a notification, back/forward) re-uses this page: forget the old one and load the new.
watch(id, () => {
  seq++
  subject.value = null
  content.value = { posts: [], courses: [], live: [] }
  assessments.value = []
  mine_.value = null
  error.value = ''
  leaving.value = false
  void load()
}, { immediate: true })
</script>

<template>
  <div v-if="subject" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/institutions/${subject.institution_id}`" class="text-body-sm underline">{{ subject.institution_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3" dir="auto">{{ subject.name_ar }}</h1>
    <p v-if="subject.path.length" class="text-body-sm mb-1" style="color: rgb(var(--md-on-surface-variant))">{{ subject.path.join(' ‹ ') }}</p>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ subject.enrolled_count }}</span> {{ pt('enrolledCount') }}</p>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <div class="mb-6 flex flex-wrap items-center gap-2">
      <template v-if="auth.role === 'student'">
        <button v-if="!subject.enrolled" class="btn-filled" data-testid="subject-enroll" @click="act(() => enroll(id))">{{ pt('enroll') }}</button>
        <template v-else>
          <span class="px-3 py-1 rounded-full text-sm font-semibold" style="background-color: rgb(var(--md-primary-container))">{{ pt('enrolledBadge') }}</span>
          <button class="btn-outlined" data-testid="subject-unenroll" @click="leaveError = ''; leaving = true">{{ pt('unenroll') }}</button>
        </template>
      </template>
      <template v-else-if="auth.role === 'teacher'">
        <button v-if="!teachingState" class="btn-filled" @click="act(() => requestTeaching(id))">{{ pt('requestTeaching') }}</button>
        <span v-else-if="teachingState === 'pending'" class="text-body-lg">{{ pt('teachingPending') }}</span>
        <span v-else-if="teachingState === 'approved'" class="text-body-lg">{{ pt('teachingApproved') }}</span>
        <template v-else>
          <span class="text-body-lg">{{ pt('teachingRejected') }}</span>
          <button class="btn-outlined" @click="act(() => requestTeaching(id))">{{ pt('teachingRetry') }}</button>
        </template>
      </template>
    </div>

    <section v-if="assessments.length" class="mb-6">
      <h2 class="text-title-md font-bold mb-2">{{ pt('assessments') }}</h2>
      <AssessmentList :items="assessments" />
    </section>

    <ContentLists :bundle="content" class="mb-6" />

    <h2 class="text-title-md font-bold mb-3">{{ pt('teachersOfSubject') }}</h2>
    <p v-if="!subject.teachers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noTeachers') }}</p>
    <ul class="space-y-3">
      <li v-for="t in subject.teachers" :key="t.id">
        <router-link :to="`/platform/teachers/${t.id}`" class="card-filled block p-4">
          <div class="font-bold" dir="auto">{{ t.full_name }}</div>
          <div v-if="t.bio" class="text-body-sm line-clamp-2" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ t.bio }}</div>
        </router-link>
      </li>
    </ul>
    <ConfirmDeleteDialog
      v-if="leaving"
      :title="`${pt('unenroll')}: ${subject.name_ar}`"
      :message="pt('mysubjUnenrollMsg')"
      :confirm-label="pt('unenroll')"
      :busy="leaveBusy"
      :error="leaveError"
      @confirm="confirmLeave"
      @close="leaving = false"
    />
  </div>
  <PageError v-else-if="error" :message="error" />
  <PageLoading v-else />
</template>
