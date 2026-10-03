<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myContent, type Bundle } from '@/api/platformContent'
import ContentLists from '@/components/platform/ContentLists.vue'

const pt = usePt()
const bundle = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')

onMounted(async () => {
  try {
    bundle.value = await myContent()
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
    </div>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <ContentLists :bundle="bundle" show-status show-edit />
  </div>
</template>
