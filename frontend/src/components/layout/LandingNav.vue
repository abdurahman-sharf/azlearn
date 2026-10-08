<script setup lang="ts">
import { usePt, type PlatformKey } from '@/i18n/platform'
import { ArrowRightOnRectangleIcon, Squares2X2Icon, UserGroupIcon, MapIcon } from '@heroicons/vue/24/outline'

// The landing page's own menu: jumps to its sections (replaces the practice/generate/search links there).
// Anchors are buttons that scroll, never `href="#id"`: the URL hash belongs to the router.
const props = defineProps<{ mode: 'top' | 'bottom' }>()
const pt = usePt()

const items = [
  { id: 'landing-features', label: 'lpNavFeatures', icon: Squares2X2Icon },
  { id: 'landing-audience', label: 'lpNavAudience', icon: UserGroupIcon },
  { id: 'landing-how', label: 'landingHowTitle', icon: MapIcon },
] as const satisfies readonly { id: string; label: PlatformKey; icon: unknown }[]

function go(id: string) {
  const el = document.getElementById(id)
  if (!el) return
  const reduce = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  el.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' })
  // move keyboard/screen-reader focus to the section's heading as well
  el.querySelector<HTMLElement>('h2')?.focus({ preventScroll: true })
}
</script>

<template>
  <nav
    v-if="props.mode === 'top'"
    :aria-label="pt('lpNavLabel')"
    class="hidden md:flex items-center gap-1 ms-6 p-1 rounded-full"
    style="background-color: rgb(var(--md-surface-container-high))"
    data-testid="landing-nav"
  >
    <button
      v-for="i in items"
      :key="i.id"
      type="button"
      class="landing-nav-btn flex items-center gap-2 px-4 h-9 rounded-full text-sm font-semibold"
      style="color: rgb(var(--md-on-surface))"
      :data-testid="'landing-nav-' + i.id.replace('landing-', '')"
      @click="go(i.id)"
    >
      <component :is="i.icon" class="w-4 h-4" aria-hidden="true" />
      <span>{{ pt(i.label) }}</span>
    </button>
  </nav>

  <nav
    v-else
    :aria-label="pt('lpNavLabel')"
    class="sm:hidden sticky bottom-0 z-30 safe-bottom"
    :style="{ backgroundColor: 'rgba(var(--md-surface-container-lowest) / 0.96)', backdropFilter: 'blur(20px)', WebkitBackdropFilter: 'blur(20px)', borderTop: '1px solid rgb(var(--md-outline-variant) / 0.6)' }"
    data-testid="landing-nav-bottom"
  >
    <div class="flex items-stretch justify-around h-16 px-1">
      <button
        v-for="i in items"
        :key="i.id"
        type="button"
        class="flex-1 flex flex-col items-center justify-center gap-0.5 py-1"
        style="color: rgb(var(--md-on-surface))"
        @click="go(i.id)"
      >
        <component :is="i.icon" class="w-5 h-5" aria-hidden="true" />
        <span class="text-[11px] font-semibold leading-tight">{{ pt(i.label) }}</span>
      </button>
      <router-link to="/auth/login" class="flex-1 flex flex-col items-center justify-center gap-0.5 py-1 no-underline" style="color: rgb(var(--md-primary))">
        <ArrowRightOnRectangleIcon class="w-5 h-5 rtl:-scale-x-100" aria-hidden="true" />
        <span class="text-[11px] font-semibold leading-tight">{{ pt('login') }}</span>
      </router-link>
    </div>
  </nav>
</template>

<style scoped>
.landing-nav-btn:hover { background-color: rgb(var(--md-primary-container)); }
</style>
