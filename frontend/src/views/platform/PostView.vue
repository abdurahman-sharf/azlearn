<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import ReportButton from '@/components/platform/ReportButton.vue'
import { getPost, updatePost, deletePost, downloadFile, type Post } from '@/api/platformContent'

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string
const post = ref<Post | null>(null)
const error = ref('')

async function load() {
  try {
    post.value = await getPost(id)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function download() {
  if (!post.value?.file) return
  try { await downloadFile(post.value.file) } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function unpublish() {
  try { await updatePost(id, { status: 'draft' }); await load() } catch (e) { error.value = platformErrorMessage(pt, e) }
}
async function remove() {
  if (!window.confirm(pt('confirmDelete'))) return
  try { await deletePost(id); router.replace('/platform') } catch (e) { error.value = platformErrorMessage(pt, e) }
}
onMounted(load)
</script>

<template>
  <div v-if="post" class="max-w-3xl mx-auto pb-8">
    <router-link :to="`/platform/subjects/${post.subject_id}`" class="text-body-sm underline">{{ post.subject_name }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight mt-3 break-words">{{ post.title }}</h1>
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
        <button class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </template>
    </div>
  </div>
  <p v-else-if="error" class="max-w-3xl mx-auto" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
</template>
