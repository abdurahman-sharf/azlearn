<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import ContentLists from '@/components/platform/ContentLists.vue'
import { subjectContent, type Bundle } from '@/api/platformContent'
import { getSubject, enroll, unenroll, myTeaching, requestTeaching, type SubjectPage, type Teaching } from '@/api/platformLearning'

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const id = route.params.id as string

const subject = ref<SubjectPage | null>(null)
const content = ref<Bundle>({ posts: [], courses: [], live: [] })
const mine = ref<Teaching | null>(null)
const error = ref('')

const teachingState = computed(() => mine.value?.status ?? null)

async function load() {
  error.value = ''
  try {
    subject.value = await getSubject(id)
    content.value = await subjectContent(id)
    if (auth.role === 'teacher') mine.value = (await myTeaching()).find(t => t.subject_id === id) ?? null
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function act(fn: () => Promise<unknown>) {
  error.value = ''
  try {
    await fn()
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

onMounted(load)
</script>

<template>
  <div v-if="subject" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/institutions/${subject.institution_id}`" class="text-body-sm underline">{{ subject.institution_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3">{{ subject.name_ar }}</h1>
    <p v-if="subject.path.length" class="text-body-sm mb-1" style="color: rgb(var(--md-on-surface-variant))">{{ subject.path.join(' ‹ ') }}</p>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ subject.enrolled_count }} {{ pt('enrolledCount') }}</p>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <div class="mb-6 flex flex-wrap items-center gap-2">
      <template v-if="auth.role === 'student'">
        <button v-if="!subject.enrolled" class="btn-filled" @click="act(() => enroll(id))">{{ pt('enroll') }}</button>
        <template v-else>
          <span class="px-3 py-1 rounded-full text-sm font-semibold" style="background-color: rgb(var(--md-primary-container))">{{ pt('enrolledBadge') }}</span>
          <button class="btn-outlined" @click="act(() => unenroll(id))">{{ pt('unenroll') }}</button>
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

    <ContentLists :bundle="content" class="mb-6" />

    <h2 class="text-title-md font-bold mb-3">{{ pt('teachersOfSubject') }}</h2>
    <p v-if="!subject.teachers.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noTeachers') }}</p>
    <ul class="space-y-3">
      <li v-for="t in subject.teachers" :key="t.id">
        <router-link :to="`/platform/teachers/${t.id}`" class="card-filled block p-4">
          <div class="font-bold">{{ t.full_name }}</div>
          <div v-if="t.bio" class="text-body-sm line-clamp-2" style="color: rgb(var(--md-on-surface-variant))">{{ t.bio }}</div>
        </router-link>
      </li>
    </ul>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
