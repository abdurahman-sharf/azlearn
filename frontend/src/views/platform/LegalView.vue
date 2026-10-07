<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useRoute } from 'vue-router'
import { platformFetch } from '@/lib/platformApi'
import { usePt } from '@/i18n/platform'

const pt = usePt()
const route = useRoute()
const slug = route.params.slug as string
const body = ref<string | null>(null)
const loading = ref(true)
const notFound = ref(false)
const title = computed(() => (slug === 'terms' ? pt('termsPage') : pt('privacyPage')))

onMounted(async () => {
  try {
    body.value = (await platformFetch<{ body: string | null }>(`/public/legal/${encodeURIComponent(slug)}`)).body
  } catch {
    notFound.value = true
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/" class="text-body-sm underline">{{ pt('backHome') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ notFound ? pt('notFound') : title }}</h1>
    <!-- The text is admin-authored but always rendered as plain text, never as HTML. -->
    <article v-if="body" class="card-filled p-5 whitespace-pre-wrap break-words text-body-lg" dir="auto" data-testid="legal-body">{{ body }}</article>
    <p v-else-if="!loading && !notFound" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="legal-empty">{{ pt('legalNotPublished') }}</p>
  </div>
</template>
