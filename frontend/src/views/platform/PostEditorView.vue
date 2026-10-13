<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { createPost, getPost, updatePost, deletePost, uploadPostFile, removePostFile, type FileInfo, type PostKind, type PubStatus } from '@/api/platformContent'
import { usableSubjects } from '@/utils/teachingCards'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'

// Draft first: a new post is saved as a draft, and is published from this edit screen once the text (and an attachment,
// if any) is ready - so nothing is announced to students half-finished.
const pt = usePt()
const i18n = useI18nStore()
const listSep = computed(() => (i18n.locale === 'ar' ? '، ' : ', '))
const route = useRoute()
const router = useRouter()
const id = ref(route.params.id as string | undefined)

const subjects = ref<MyTeaching[]>([])
const subjectId = ref('')
const kind = ref<PostKind>('article')
const title = ref('')
const body = ref('')
const status = ref<PubStatus>('draft')
const file = ref<FileInfo | null>(null)
const error = ref('')
const msg = ref('')
const busy = ref(false)
const fileBusy = ref(false)
const loaded = ref(false)
const isEdit = computed(() => !!id.value)

const { asking: leaving, answer: answerLeave, markClean } = useUnsavedGuard(() => ({ subjectId: subjectId.value, kind: kind.value, title: title.value, body: body.value }))

// what is still missing before the post can be published
const missing = computed(() => {
  const out: string[] = []
  if (!title.value.trim()) out.push(pt('edMissingTitle'))
  if (!body.value.trim()) out.push(pt('edMissingBody'))
  return out
})

onMounted(async () => {
  try {
    subjects.value = usableSubjects(await myTeaching()).filter((t) => t.status === 'approved')
    if (id.value) {
      const p = await getPost(id.value)
      subjectId.value = p.subject_id; kind.value = p.kind; title.value = p.title
      body.value = p.body ?? ''; status.value = p.status; file.value = p.file
    } else {
      const wanted = route.query.subject as string | undefined
      subjectId.value = subjects.value.find((s) => s.subject_id === wanted)?.subject_id ?? subjects.value[0]?.subject_id ?? ''
    }
    loaded.value = true
    markClean()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

/** Saves the text. `next` is the status to end up in: omitted = keep the current one (a new post is always a draft). */
async function save(next?: PubStatus) {
  if (busy.value) return
  error.value = msg.value = ''
  busy.value = true
  try {
    if (id.value) {
      const p = await updatePost(id.value, { kind: kind.value, title: title.value, body: body.value, status: next ?? status.value })
      status.value = p.status
      markClean()
      msg.value = next === 'published' ? pt('edPublished') : next === 'draft' ? pt('edUnpublished') : pt('saved')
    } else {
      const p = await createPost({ subject_id: subjectId.value, kind: kind.value, title: title.value, body: body.value, status: 'draft' })
      id.value = p.id
      status.value = p.status
      markClean() // before navigating: the guard must not ask about the text that was just saved
      await router.replace(`/platform/posts/${p.id}/edit`)
      msg.value = pt('edDraftSaved')
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
  if (!f || !id.value || fileBusy.value) return
  error.value = msg.value = ''
  fileBusy.value = true
  try {
    file.value = await uploadPostFile(id.value, f)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    fileBusy.value = false
  }
}

async function dropFile() {
  if (!id.value || fileBusy.value) return
  fileBusy.value = true
  try {
    await removePostFile(id.value)
    file.value = null
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    fileBusy.value = false
  }
}

// --- delete --------------------------------------------------------------------------------------------------------------
const deleting = ref(false)
const deleteBusy = ref(false)
const deleteError = ref('')

async function remove() {
  if (!id.value || deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await deletePost(id.value)
    markClean() // the page is going away on purpose
    deleting.value = false
    await router.replace('/platform/my-content')
  } catch (e) {
    deleteError.value = platformErrorMessage(pt, e)
  } finally {
    deleteBusy.value = false
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8" data-testid="post-editor">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newPost') }}</h1>
    <p v-if="loaded && !subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="editor-no-subjects">
      {{ pt('noApprovedSubjects') }} <router-link to="/platform/teaching" class="underline font-semibold" data-testid="editor-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </p>
    <p v-else-if="!loaded && error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>

    <form v-else-if="loaded" class="card-elevated p-5 space-y-4" @submit.prevent="save()">
      <p v-if="isEdit" class="text-body-sm" data-testid="post-status">
        <span class="font-semibold">{{ pt('edStatus') }}:</span> {{ status === 'published' ? pt('statusPublished') : pt('statusDraft') }}
      </p>
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full" data-testid="post-subject">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <div role="group" :aria-label="pt('postKind')" class="flex gap-2">
        <button type="button" class="flex-1" :class="kind === 'article' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="kind === 'article'" data-testid="post-kind-article" @click="kind = 'article'">{{ pt('kindArticle') }}</button>
        <button type="button" class="flex-1" :class="kind === 'summary' ? 'btn-filled' : 'btn-outlined'" :aria-pressed="kind === 'summary'" data-testid="post-kind-summary" @click="kind = 'summary'">{{ pt('kindSummary') }}</button>
      </div>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="post-title" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('body') }}</span>
        <textarea v-model="body" required maxlength="20000" rows="10" dir="auto" class="input-outlined mt-1 w-full" data-testid="post-body"></textarea>
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ body.length }} / 20000</span></span>
      </label>
      <p v-if="!isEdit" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="post-draft-hint">{{ pt('edNewPostHint') }}</p>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status" data-testid="editor-msg">{{ msg }}</p>
      <p v-if="isEdit && status === 'draft' && missing.length" id="post-publish-missing" class="text-body-sm" role="note" data-testid="post-publish-missing">{{ pt('edPublishNeeds') }} {{ missing.join(listSep) }}</p>
      <div class="flex flex-wrap gap-2">
        <button type="submit" class="btn-filled" :disabled="busy" data-testid="post-save">{{ isEdit && status === 'published' ? pt('edSaveChanges') : pt('edSaveDraft') }}</button>
        <button v-if="isEdit && status === 'draft'" type="button" class="btn-tonal" :disabled="busy || missing.length > 0" :aria-describedby="missing.length ? 'post-publish-missing' : undefined" data-testid="post-publish" @click="save('published')">{{ pt('publish') }}</button>
        <button v-if="isEdit && status === 'published'" type="button" class="btn-outlined" :disabled="busy" data-testid="post-unpublish" @click="save('draft')">{{ pt('unpublish') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" :disabled="busy" data-testid="post-delete" @click="deleteError = ''; deleting = true">{{ pt('del') }}</button>
      </div>
    </form>

    <section v-if="isEdit" class="card-filled p-4 mt-4 space-y-2" aria-labelledby="post-attachment-title">
      <h2 id="post-attachment-title" class="font-semibold">{{ pt('attachment') }}</h2>
      <div v-if="file" class="flex items-center gap-2">
        <span class="flex-1 min-w-0 truncate" dir="auto" data-testid="post-file-name">{{ file.name }}</span>
        <button type="button" class="btn-text" :disabled="fileBusy" :aria-label="`${pt('removeFile')}: ${file.name}`" data-testid="post-file-remove" @click="dropFile">{{ pt('removeFile') }}</button>
      </div>
      <label class="block text-body-sm">
        {{ pt('attachFile') }}
        <input type="file" accept=".pdf,.docx,.pptx,.xlsx,.png,.jpg,.jpeg,.txt" class="mt-1 block" :disabled="fileBusy" data-testid="post-file-input" @change="onFile" />
      </label>
      <p v-if="fileBusy" class="text-body-sm" role="status">{{ pt('edUploading') }}</p>
    </section>

    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('del')}: ${title}`"
      :message="status === 'published' ? `${pt('edDeletePostMsg')} ${pt('edDeletePublished')}` : pt('edDeletePostMsg')"
      :counts="[{ label: pt('attachment'), value: file ? 1 : 0 }]"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="remove"
      @close="deleting = false"
    />
    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
  </div>
</template>
