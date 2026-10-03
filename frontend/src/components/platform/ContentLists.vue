<script setup lang="ts">
import { computed } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, type PlatformKey } from '@/i18n/platform'
import type { Bundle } from '@/api/platformContent'

const props = defineProps<{ bundle: Bundle; showStatus?: boolean; showEdit?: boolean; hideLive?: boolean }>()
const pt = usePt()
const i18n = useI18nStore()

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const statusKey: Record<string, PlatformKey> = { draft: 'statusDraft', published: 'statusPublished' }
const empty = computed(() => !props.bundle.posts.length && !props.bundle.courses.length && !(props.bundle.live.length && !props.hideLive))
</script>

<template>
  <div class="space-y-6">
    <p v-if="empty" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noContent') }}</p>

    <section v-if="bundle.live.length && !hideLive">
      <h3 class="text-title-md font-bold mb-2">{{ pt('liveSessions') }}</h3>
      <ul class="space-y-2">
        <li v-for="l in bundle.live" :key="l.id" class="card-filled p-3" :class="{ 'opacity-60': l.status === 'cancelled' }">
          <div class="flex items-start gap-2">
            <div class="flex-1 min-w-0">
              <div class="font-bold break-words">{{ l.title }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
                {{ fmt(l.starts_at) }} · {{ l.duration_min }} {{ pt('minutesShort') }} · {{ l.subject_name }} · {{ pt('by') }} {{ l.teacher_name }}
              </div>
            </div>
            <span v-if="l.status === 'cancelled'" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt('cancelled') }}</span>
          </div>
          <div class="flex flex-wrap gap-2 mt-2">
            <a v-if="l.status === 'scheduled'" :href="l.join_url" target="_blank" rel="noopener noreferrer" class="btn-tonal">{{ pt('join') }}</a>
            <router-link v-if="showEdit" :to="`/platform/live/${l.id}/edit`" class="btn-text">{{ pt('edit') }}</router-link>
          </div>
        </li>
      </ul>
    </section>

    <section v-if="bundle.courses.length">
      <h3 class="text-title-md font-bold mb-2">{{ pt('courses') }}</h3>
      <ul class="space-y-2">
        <li v-for="c in bundle.courses" :key="c.id" class="card-filled p-3 flex items-center gap-2">
          <router-link :to="`/platform/courses/${c.id}`" class="flex-1 min-w-0">
            <div class="font-bold break-words">{{ c.title }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ c.lesson_count }} {{ pt('lessonsCount') }} · {{ c.subject_name }} · {{ pt('by') }} {{ c.teacher_name }}</div>
          </router-link>
          <span v-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[c.status]!) }}</span>
          <router-link v-if="showEdit" :to="`/platform/courses/${c.id}/edit`" class="btn-text">{{ pt('edit') }}</router-link>
        </li>
      </ul>
    </section>

    <section v-if="bundle.posts.length">
      <h3 class="text-title-md font-bold mb-2">{{ pt('posts') }}</h3>
      <ul class="space-y-2">
        <li v-for="p in bundle.posts" :key="p.id" class="card-filled p-3 flex items-center gap-2">
          <router-link :to="`/platform/posts/${p.id}`" class="flex-1 min-w-0">
            <div class="font-bold break-words">{{ p.title }}</div>
            <div class="text-body-sm line-clamp-2" style="color: rgb(var(--md-on-surface-variant))">{{ p.excerpt }}</div>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ p.kind === 'summary' ? pt('kindSummary') : pt('kindArticle') }} · {{ p.subject_name }} · {{ pt('by') }} {{ p.teacher_name }}</div>
          </router-link>
          <span v-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[p.status]!) }}</span>
          <router-link v-if="showEdit" :to="`/platform/posts/${p.id}/edit`" class="btn-text">{{ pt('edit') }}</router-link>
        </li>
      </ul>
    </section>
  </div>
</template>
