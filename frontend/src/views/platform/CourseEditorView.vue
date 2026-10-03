<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type Teaching } from '@/api/platformLearning'
import { createCourse, getCourse, updateCourse, deleteCourse, addLesson, deleteLesson, moveLesson, type Lesson, type PubStatus } from '@/api/platformContent'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const id = ref(route.params.id as string | undefined)

const subjects = ref<Teaching[]>([])
const subjectId = ref('')
const title = ref('')
const description = ref('')
const status = ref<PubStatus>('draft')
const lessons = ref<Lesson[]>([])
const error = ref('')
const msg = ref('')
const isEdit = computed(() => !!id.value)

const lTitle = ref('')
const lUrl = ref('')
const lSection = ref('')

async function load() {
  if (!id.value) return
  const c = await getCourse(id.value)
  subjectId.value = c.subject_id; title.value = c.title; description.value = c.description ?? ''
  status.value = c.status; lessons.value = c.lessons
}

onMounted(async () => {
  try {
    subjects.value = (await myTeaching()).filter(t => t.status === 'approved')
    if (id.value) await load()
    else subjectId.value = (route.query.subject as string) || subjects.value[0]?.subject_id || ''
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

async function run(fn: () => Promise<unknown>) {
  error.value = msg.value = ''
  try {
    await fn()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

const save = () => run(async () => {
  if (id.value) {
    await updateCourse(id.value, { title: title.value, description: description.value, status: status.value })
  } else {
    const c = await createCourse({ subject_id: subjectId.value, title: title.value, description: description.value || undefined, status: status.value })
    id.value = c.id
    router.replace(`/platform/courses/${c.id}/edit`)
  }
  msg.value = pt('saved')
})

const addL = () => run(async () => {
  await addLesson(id.value!, { title: lTitle.value, video_url: lUrl.value, section: lSection.value || undefined })
  lTitle.value = lUrl.value = ''
  await load()
})
const move = (l: Lesson, d: 'up' | 'down') => run(async () => { await moveLesson(l.id, d); await load() })
const delL = (l: Lesson) => window.confirm(pt('confirmDelete')) && run(async () => { await deleteLesson(l.id); await load() })
const delCourse = () => window.confirm(pt('confirmDelete')) && run(async () => { await deleteCourse(id.value!); router.replace('/platform/my-content') })
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newCourse') }}</h1>
    <p v-if="!subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noApprovedSubjects') }}</p>

    <form v-else class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="3" class="input-outlined mt-1 w-full"></textarea>
      </label>
      <select v-model="status" class="input-outlined w-full" :aria-label="pt('statusDraft')">
        <option value="draft">{{ pt('statusDraft') }}</option>
        <option value="published">{{ pt('statusPublished') }}</option>
      </select>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <div class="flex gap-2">
        <button type="submit" class="btn-filled">{{ pt('save') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" @click="delCourse">{{ pt('del') }}</button>
      </div>
    </form>

    <section v-if="isEdit" class="mt-6">
      <h2 class="text-title-md font-bold mb-3">{{ pt('lessons') }}</h2>
      <ol class="space-y-2 mb-4">
        <li v-for="(l, i) in lessons" :key="l.id" class="card-filled p-3">
          <div class="flex items-center gap-2">
            <span class="font-bold">{{ i + 1 }}.</span>
            <span class="flex-1 min-w-0 break-words">{{ l.title }}<span v-if="l.section" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"> · {{ l.section }}</span></span>
          </div>
          <div class="flex flex-wrap gap-x-1 mt-1">
            <button class="btn-text" :disabled="i === 0" @click="move(l, 'up')">{{ pt('moveUp') }}</button>
            <button class="btn-text" :disabled="i === lessons.length - 1" @click="move(l, 'down')">{{ pt('moveDown') }}</button>
            <button class="btn-text" @click="delL(l)">{{ pt('del') }}</button>
          </div>
        </li>
      </ol>
      <form class="card-elevated p-4 space-y-3" @submit.prevent="addL">
        <div class="font-semibold">{{ pt('addLesson') }}</div>
        <input v-model="lTitle" required maxlength="200" :placeholder="pt('lessonTitle')" class="input-outlined w-full" />
        <input v-model="lUrl" required type="url" dir="ltr" :placeholder="pt('videoUrl')" class="input-outlined w-full" />
        <input v-model="lSection" maxlength="120" :placeholder="pt('section')" class="input-outlined w-full" />
        <button type="submit" class="btn-tonal">{{ pt('add') }}</button>
      </form>
    </section>
  </div>
</template>
