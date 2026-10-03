<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type Teaching } from '@/api/platformLearning'
import { createPost, getPost, updatePost, deletePost, uploadPostFile, removePostFile, type FileInfo, type PostKind, type PubStatus } from '@/api/platformContent'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const id = ref(route.params.id as string | undefined)

const subjects = ref<Teaching[]>([])
const subjectId = ref('')
const kind = ref<PostKind>('article')
const title = ref('')
const body = ref('')
const status = ref<PubStatus>('draft')
const file = ref<FileInfo | null>(null)
const error = ref('')
const msg = ref('')
const busy = ref(false)
const isEdit = computed(() => !!id.value)

onMounted(async () => {
  try {
    subjects.value = (await myTeaching()).filter(t => t.status === 'approved')
    if (id.value) {
      const p = await getPost(id.value)
      subjectId.value = p.subject_id; kind.value = p.kind; title.value = p.title
      body.value = p.body ?? ''; status.value = p.status; file.value = p.file
    } else {
      subjectId.value = (route.query.subject as string) || subjects.value[0]?.subject_id || ''
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

async function save() {
  error.value = msg.value = ''
  busy.value = true
  try {
    if (id.value) {
      await updatePost(id.value, { kind: kind.value, title: title.value, body: body.value, status: status.value })
      msg.value = pt('saved')
    } else {
      const p = await createPost({ subject_id: subjectId.value, kind: kind.value, title: title.value, body: body.value, status: status.value })
      id.value = p.id
      router.replace(`/platform/posts/${p.id}/edit`)
      msg.value = pt('saved')
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}

async function onFile(ev: Event) {
  const input = ev.target as HTMLInputElement
  const f = input.files?.[0]
  input.value = ''
  if (!f || !id.value) return
  error.value = ''
  try {
    file.value = await uploadPostFile(id.value, f)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function dropFile() {
  if (!id.value) return
  try {
    await removePostFile(id.value)
    file.value = null
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove() {
  if (!id.value || !window.confirm(pt('confirmDelete'))) return
  try {
    await deletePost(id.value)
    router.replace('/platform/my-content')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newPost') }}</h1>
    <p v-if="!subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noApprovedSubjects') }}</p>

    <form v-else class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <div class="flex gap-2">
        <button type="button" class="flex-1" :class="kind === 'article' ? 'btn-filled' : 'btn-outlined'" @click="kind = 'article'">{{ pt('kindArticle') }}</button>
        <button type="button" class="flex-1" :class="kind === 'summary' ? 'btn-filled' : 'btn-outlined'" @click="kind = 'summary'">{{ pt('kindSummary') }}</button>
      </div>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('body') }}</span>
        <textarea v-model="body" required maxlength="20000" rows="10" class="input-outlined mt-1 w-full"></textarea>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ status === 'published' ? pt('statusPublished') : pt('statusDraft') }}</span>
        <select v-model="status" class="input-outlined mt-1 w-full">
          <option value="draft">{{ pt('statusDraft') }}</option>
          <option value="published">{{ pt('statusPublished') }}</option>
        </select>
      </label>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <div class="flex flex-wrap gap-2">
        <button type="submit" class="btn-filled" :disabled="busy">{{ pt('save') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </div>
    </form>

    <section v-if="isEdit" class="card-filled p-4 mt-4 space-y-2">
      <div class="font-semibold">{{ pt('attachment') }}</div>
      <div v-if="file" class="flex items-center gap-2">
        <span class="flex-1 min-w-0 truncate">{{ file.name }}</span>
        <button class="btn-text" @click="dropFile">{{ pt('removeFile') }}</button>
      </div>
      <label class="block text-body-sm">
        {{ pt('attachFile') }}
        <input type="file" accept=".pdf,.docx,.pptx,.xlsx,.png,.jpg,.jpeg,.txt" class="mt-1 block" @change="onFile" />
      </label>
    </section>
  </div>
</template>
