<script setup lang="ts">
import { usePt, type PlatformKey } from '@/i18n/platform'

// What a page shows while its data is on the way (instead of a blank screen): a skeleton of a heading and a few rows
// and a spinner, announced once to screen readers as "Loading…" (role=status + aria-busy). The page replaces it with its
// content or with PageError.
withDefaults(defineProps<{ label?: PlatformKey; rows?: number }>(), { label: 'loading', rows: 3 })
const pt = usePt()
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8" role="status" aria-busy="true" data-testid="page-loading">
    <div class="flex items-center gap-3 mb-5">
      <span class="page-loading-spinner w-6 h-6 shrink-0 rounded-full border-[3px]" aria-hidden="true"></span>
      <span class="text-body-lg font-semibold" style="color: rgb(var(--md-on-surface-variant))">{{ pt(label) }}</span>
    </div>
    <div aria-hidden="true" class="space-y-3">
      <div class="page-loading-bar h-8 w-2/3 max-w-sm rounded-xl"></div>
      <div v-for="i in rows" :key="i" class="page-loading-bar h-16 w-full rounded-2xl"></div>
    </div>
  </div>
</template>

<style scoped>
.page-loading-spinner {
  border-color: rgb(var(--md-surface-container-highest));
  border-top-color: rgb(var(--md-primary));
  animation: page-loading-spin 0.9s linear infinite;
}
.page-loading-bar {
  background-color: rgb(var(--md-surface-container-high));
  animation: page-loading-pulse 1.4s ease-in-out infinite;
}
@keyframes page-loading-spin { to { transform: rotate(360deg); } }
@keyframes page-loading-pulse { 50% { opacity: 0.55; } }
@media (prefers-reduced-motion: reduce) {
  .page-loading-spinner, .page-loading-bar { animation: none; }
}
</style>
