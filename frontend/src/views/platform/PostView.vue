<script setup lang="ts">
import { ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useRouteParam } from '@/composables/useRouteParam'
import ReportButton from '@/components/platform/ReportButton.vue'
import BookmarkButton from '@/components/platform/BookmarkButton.vue'
import { getPost, updatePost, deletePost, downloadFile, type Post } from '@/api/platformContent'
import PageError from '@/components/platform/PageError.vue'
import PageLoading from '@/components/platform/PageLoading.vue'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()
const id = useRouteParam('id')
const post = ref<Post | null>(null)
const error = ref('')

// Answers to the latest load only: a slow answer for the post we just left must not land on the one we are opening.
let seq = 0
async function load() {
  const mine = ++seq
  try {
    const p = await getPost(id.value)
    if (mine === seq) { post.value = p; error.value = '' }
  } catch (e) {
    if (mine === seq) error.value = platformErrorMessage(pt, e)
  }
}
async function download() {
  if (!post.value?.file) return
  try { await downloadFile(post.value.file) } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function unpublish() {
  try { await updatePost(id.value, { status: 'draft' }); await load() } catch (e) { error.value = platformErrorMessage(pt, e) }
}
const deleting = ref(false)
const deleteBusy = ref(false)
const deleteError = ref('')
async function remove() {
  if (deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await deletePost(id.value)
    deleting.value = false
    await router.replace('/platform')
  } catch (e) {
    deleteError.value = platformErrorMessage(pt, e)
  } finally {
    deleteBusy.value = false
  }
}
// Moving to another post (a link, a notification, back/forward) re-uses this page: forget the old one and load the new.
watch(id, () => {
  seq++
  post.value = null
  error.value = ''
  deleting.value = false
  void load()
}, { immediate: true })
</script>

<template>
  <div v-if="post" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${post.subject_id}`" class="text-body-sm underline">{{ post.subject_name }}</router-link>
    <div class="flex flex-wrap items-start gap-3 mt-3">
      <h1 class="flex-1 min-w-[12rem] text-display-sm font-bold tracking-tight break-words" dir="auto">{{ post.title }}</h1>
      <BookmarkButton v-if="auth.role === 'student'" target-type="post" :target-id="post.id" testid="post-bookmark" />
    </div>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">
      {{ post.kind === 'summary' ? pt('kindSummary') : pt('kindArticle') }} · {{ pt('by') }}
      <router-link :to="`/platform/teachers/${post.teacher_id}`" class="underline">{{ post.teacher_name }}</router-link>
      <template v-if="post.status === 'draft'"> · {{ pt('statusDraft') }}</template>
    </p>
    <!-- Plain text only: never rendered as HTML. -->
    <article class="text-body-lg whitespace-pre-wrap break-words mb-6" dir="auto">{{ post.body }}</article>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <div v-if="post.file" class="card-filled p-3 flex items-center gap-2 mb-4">
      <span class="flex-1 min-w-0 truncate">{{ post.file.name }}</span>
      <button class="btn-tonal" @click="download">{{ pt('download') }}</button>
    </div>
    <div class="flex flex-wrap gap-2">
      <ReportButton v-if="auth.profile?.id !== post.teacher_id" target-type="post" :target-id="post.id" />
      <router-link v-if="auth.profile?.id === post.teacher_id" :to="`/platform/posts/${post.id}/edit`" class="btn-outlined">{{ pt('edit') }}</router-link>
      <template v-if="auth.role === 'admin'">
        <button v-if="post.status === 'published'" class="btn-outlined" @click="unpublish">{{ pt('unpublish') }}</button>
        <button class="btn-outlined" data-testid="post-admin-delete" @click="deleteError = ''; deleting = true">{{ pt('del') }}</button>
      </template>
    </div>
    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('del')}: ${post.title}`"
      :message="post.status === 'published' ? `${pt('edDeletePostMsg')} ${pt('edDeletePublished')}` : pt('edDeletePostMsg')"
      :counts="[{ label: pt('attachment'), value: post.file ? 1 : 0 }]"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="remove"
      @close="deleting = false"
    />
  </div>
  <PageError v-else-if="error" :message="error" />
  <PageLoading v-else />
</template>
