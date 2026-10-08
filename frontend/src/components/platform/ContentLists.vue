<script setup lang="ts">
import { computed } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, type PlatformKey } from '@/i18n/platform'
import type { Bundle } from '@/api/platformContent'
import OwnerStateChip from './OwnerStateChip.vue'
import HiddenWhy from './HiddenWhy.vue'

// Posts, courses and live sessions. `own` is the teacher's view of their own items (My content): no "by <me>", one state
// chip that says "hidden: <reason>" instead of a green "published" when students cannot see the item, no "Join" for a
// session that is over, and copy buttons for posts and courses. The public lists leave `own` off and look as before.
const props = defineProps<{
  bundle: Bundle
  showStatus?: boolean
  showEdit?: boolean
  hideLive?: boolean
  own?: boolean
  /** with `own`: the page shows its own empty state */
  hideEmpty?: boolean
  /** with `own`: the id of the item being copied right now (its button is disabled) */
  busyId?: string
}>()
const emit = defineEmits<{ duplicate: [kind: 'post' | 'course', id: string, title: string] }>()
const pt = usePt()
const i18n = useI18nStore()

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const statusKey: Record<string, PlatformKey> = { draft: 'statusDraft', published: 'statusPublished' }
const empty = computed(() => !props.bundle.posts.length && !props.bundle.courses.length && !(props.bundle.live.length && !props.hideLive))
</script>

<template>
  <div class="space-y-6">
    <p v-if="empty && !hideEmpty" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noContent') }}</p>

    <section v-if="bundle.live.length && !hideLive" data-testid="content-live">
      <h2 class="text-title-md font-bold mb-2">{{ pt('liveSessions') }}</h2>
      <ul class="space-y-2">
        <li v-for="l in bundle.live" :key="l.id" class="card-filled p-3" :data-testid="`content-live-${l.id}`">
          <div class="flex flex-wrap items-start gap-2">
            <div class="flex-1 min-w-[9rem]">
              <div class="font-bold break-words" dir="auto" :class="{ 'line-through': !own && l.status === 'cancelled' }">{{ l.title }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
                {{ fmt(l.starts_at) }} · <span dir="ltr" class="inline-block">{{ l.duration_min }}</span> {{ pt('minutesShort') }} · {{ l.subject_name }}<template v-if="!own"> · {{ pt('by') }} {{ l.teacher_name }}</template>
              </div>
            </div>
            <OwnerStateChip v-if="own" :row="{ status: l.status, visible: l.visible, hidden_reason: l.hidden_reason, ended: l.ended }" />
            <span v-else-if="l.status === 'cancelled'" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt('cancelled') }}</span>
          </div>
          <HiddenWhy v-if="own && l.status === 'scheduled' && !l.ended" :reason="l.hidden_reason" class="mt-1" />
          <div class="flex flex-wrap gap-2 mt-2">
            <a v-if="l.status === 'scheduled' && !l.ended" :href="l.join_url" target="_blank" rel="noopener noreferrer" class="btn-tonal" :aria-label="`${pt('join')}: ${l.title}`" :data-testid="`content-join-${l.id}`">{{ pt('join') }}</a>
            <router-link v-if="showEdit" :to="`/platform/live/${l.id}/edit`" class="btn-text" :aria-label="`${pt('edit')}: ${l.title}`">{{ pt('edit') }}</router-link>
          </div>
        </li>
      </ul>
    </section>

    <section v-if="bundle.courses.length" data-testid="content-courses">
      <h2 class="text-title-md font-bold mb-2">{{ pt('courses') }}</h2>
      <ul class="space-y-2">
        <li v-for="c in bundle.courses" :key="c.id" class="card-filled p-3" :data-testid="`content-course-${c.id}`">
          <div class="flex flex-wrap items-center gap-2">
            <router-link :to="`/platform/courses/${c.id}`" class="flex-1 min-w-[9rem]">
              <div class="font-bold break-words" dir="auto">{{ c.title }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">{{ c.lesson_count }}</span> {{ pt('lessonsCount') }} · {{ c.subject_name }}<template v-if="!own"> · {{ pt('by') }} {{ c.teacher_name }}</template></div>
            </router-link>
            <OwnerStateChip v-if="own" :row="{ status: c.status, visible: c.visible, hidden_reason: c.hidden_reason }" />
            <span v-else-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[c.status]!) }}</span>
            <router-link v-if="showEdit" :to="`/platform/courses/${c.id}/edit`" class="btn-text" :aria-label="`${pt('edit')}: ${c.title}`">{{ pt('edit') }}</router-link>
            <button v-if="own" type="button" class="btn-text" :disabled="busyId === c.id" :aria-label="`${pt('hubDuplicate')}: ${c.title}`" :data-testid="`duplicate-course-${c.id}`" @click="emit('duplicate', 'course', c.id, c.title)">{{ pt('hubDuplicate') }}</button>
          </div>
          <HiddenWhy v-if="own && c.status === 'published'" :reason="c.hidden_reason" class="mt-1" />
        </li>
      </ul>
    </section>

    <section v-if="bundle.posts.length" data-testid="content-posts">
      <h2 class="text-title-md font-bold mb-2">{{ pt('posts') }}</h2>
      <ul class="space-y-2">
        <li v-for="p in bundle.posts" :key="p.id" class="card-filled p-3" :data-testid="`content-post-${p.id}`">
          <div class="flex flex-wrap items-center gap-2">
            <router-link :to="`/platform/posts/${p.id}`" class="flex-1 min-w-[9rem]">
              <div class="font-bold break-words" dir="auto">{{ p.title }}</div>
              <div class="text-body-sm line-clamp-2" dir="auto" style="color: rgb(var(--md-on-surface-variant))">{{ p.excerpt }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ p.kind === 'summary' ? pt('kindSummary') : pt('kindArticle') }} · {{ p.subject_name }}<template v-if="!own"> · {{ pt('by') }} {{ p.teacher_name }}</template></div>
            </router-link>
            <OwnerStateChip v-if="own" :row="{ status: p.status, visible: p.visible, hidden_reason: p.hidden_reason }" />
            <span v-else-if="showStatus" class="text-xs font-semibold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[p.status]!) }}</span>
            <router-link v-if="showEdit" :to="`/platform/posts/${p.id}/edit`" class="btn-text" :aria-label="`${pt('edit')}: ${p.title}`">{{ pt('edit') }}</router-link>
            <button v-if="own" type="button" class="btn-text" :disabled="busyId === p.id" :aria-label="`${pt('hubDuplicate')}: ${p.title}`" :data-testid="`duplicate-post-${p.id}`" @click="emit('duplicate', 'post', p.id, p.title)">{{ pt('hubDuplicate') }}</button>
          </div>
          <HiddenWhy v-if="own && p.status === 'published'" :reason="p.hidden_reason" class="mt-1" />
        </li>
      </ul>
    </section>
  </div>
</template>
