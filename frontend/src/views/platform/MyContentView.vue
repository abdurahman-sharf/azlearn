<script setup lang="ts">
import { computed, ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useTeacherStats } from '@/lib/teacherStats'
import { myContent, type Bundle } from '@/api/platformContent'
import AssessmentList from '@/components/platform/AssessmentList.vue'
import { myAssessments, type AssessmentInfo } from '@/api/platformExams'
import ContentLists from '@/components/platform/ContentLists.vue'

const pt = usePt()
const bundle = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')
const assessments = ref<AssessmentInfo[]>([])

// The admin can switch exam creation off; the button then says why instead of leading to an error.
const { stats, refresh } = useTeacherStats()
const examsOff = computed(() => stats.value?.can_create_exams === false)

onMounted(async () => {
  refresh()
  try {
    const [b, a] = await Promise.all([myContent(), myAssessments()])
    bundle.value = b
    assessments.value = a
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myContent') }}</h1>
    <div class="flex flex-wrap gap-2" :class="examsOff ? 'mb-2' : 'mb-5'">
      <router-link to="/platform/posts/new" class="btn-filled" data-testid="new-post">{{ pt('newPost') }}</router-link>
      <router-link to="/platform/courses/new" class="btn-tonal" data-testid="new-course">{{ pt('newCourse') }}</router-link>
      <router-link to="/platform/live/new" class="btn-tonal" data-testid="new-live">{{ pt('newLive') }}</router-link>
      <router-link v-if="!examsOff" to="/platform/assessments/new" class="btn-tonal" data-testid="new-exam">{{ pt('newAssessment') }}</router-link>
      <button v-else type="button" class="btn-tonal" disabled aria-describedby="exams-off-reason" data-testid="new-exam-disabled">{{ pt('newAssessment') }}</button>
    </div>
    <p v-if="examsOff" id="exams-off-reason" class="text-body-sm mb-5" style="color: rgb(var(--md-on-surface-variant))" data-testid="exams-off-reason">{{ pt('examsOffReason') }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <section v-if="assessments.length" class="mb-6">
      <h2 class="text-title-md font-bold mb-2">{{ pt('assessments') }}</h2>
      <AssessmentList :items="assessments" show-status show-edit />
    </section>
    <ContentLists :bundle="bundle" show-status show-edit />
  </div>
</template>
