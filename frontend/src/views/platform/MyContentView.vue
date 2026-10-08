<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myContent, type Bundle } from '@/api/platformContent'
import AssessmentList from '@/components/platform/AssessmentList.vue'
import { myAssessments, type AssessmentInfo } from '@/api/platformExams'
import ContentLists from '@/components/platform/ContentLists.vue'

const pt = usePt()
const bundle = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')
const assessments = ref<AssessmentInfo[]>([])

onMounted(async () => {
  try {
    bundle.value = await myContent()
    assessments.value = await myAssessments()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myContent') }}</h1>
    <div class="flex flex-wrap gap-2 mb-5">
      <router-link to="/platform/posts/new" class="btn-filled">{{ pt('newPost') }}</router-link>
      <router-link to="/platform/courses/new" class="btn-tonal">{{ pt('newCourse') }}</router-link>
      <router-link to="/platform/live/new" class="btn-tonal">{{ pt('newLive') }}</router-link>
      <router-link to="/platform/assessments/new" class="btn-tonal">{{ pt('newAssessment') }}</router-link>
    </div>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <section v-if="assessments.length" class="mb-6">
      <h2 class="text-title-md font-bold mb-2">{{ pt('assessments') }}</h2>
      <AssessmentList :items="assessments" show-status show-edit />
    </section>
    <ContentLists :bundle="bundle" show-status show-edit />
  </div>
</template>
