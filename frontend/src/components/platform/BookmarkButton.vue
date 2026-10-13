<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { BookmarkIcon as BookmarkOutline } from '@heroicons/vue/24/outline'
import { BookmarkIcon as BookmarkSolid } from '@heroicons/vue/24/solid'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { useBookmarks } from '@/lib/bookmarks'
import type { BookmarkType } from '@/api/platformStudent'

// "Save" for a post or a course (students only - the page decides whether to render it). The state comes from the shared
// list (read once per session); a click changes it at once and undoes it if the server refuses. The label stays "Save" in
// both states (a pressed toggle button must not also change its name); the pressed state is `aria-pressed` and the icon
// (outline / filled), so it never depends on colour.
const props = defineProps<{ targetType: BookmarkType; targetId: string; testid: string }>()
const pt = usePt()
const bm = useBookmarks()
const error = ref('')

onMounted(() => { void bm.ensureLoaded() })

const on = computed(() => bm.has(props.targetType, props.targetId))
/** the list is still being read: whether this item is saved is not known yet (a failed read leaves the button usable) */
const waiting = computed(() => !bm.loaded.value && !bm.failed.value)
const busy = computed(() => bm.busy(props.targetType, props.targetId))

async function toggle() {
  if (busy.value || waiting.value) return
  error.value = ''
  try {
    await bm.set(props.targetType, props.targetId, !on.value)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="inline-flex flex-col items-start gap-1">
    <button
      type="button"
      class="inline-flex items-center gap-2"
      :class="on ? 'btn-tonal' : 'btn-outlined'"
      :aria-pressed="on"
      :aria-busy="busy || waiting ? 'true' : undefined"
      :disabled="waiting"
      :data-testid="props.testid"
      @click="toggle"
    >
      <component :is="on ? BookmarkSolid : BookmarkOutline" class="w-5 h-5" aria-hidden="true" />
      <span>{{ pt('bmSave') }}</span>
    </button>
    <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" :data-testid="props.testid + '-error'">{{ error }}</p>
  </div>
</template>
