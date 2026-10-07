<script setup lang="ts">
import { computed } from 'vue'
import { useBrandingStore } from '@/stores/branding'
import { usePt } from '@/i18n/platform'

const props = withDefaults(defineProps<{ size?: number }>(), { size: 48 })
const brand = useBrandingStore()
const pt = usePt()
const name = computed(() => brand.name ?? pt('defaultPlatformName'))
// Until the admin sets a custom logo/name the identity is the official azlearn lockup
// (public/azlearn-logo.png); a custom logo is shown next to the name instead.
const wordmark = computed(() => !brand.logo && !brand.name)
</script>

<template>
  <span class="inline-flex items-center gap-3 align-middle" data-testid="brand-mark">
    <template v-if="wordmark">
      <!-- The official azlearn lockup has dark-blue letters, so it sits on a white chip (needed in dark mode). -->
      <span class="inline-flex items-center rounded-xl bg-white px-2 py-1 shrink-0" data-testid="brand-wordmark" :aria-label="name">
        <img src="/azlearn-logo.png" alt="" :style="{ height: size * 1.1 + 'px', width: 'auto' }" />
      </span>
      <span class="sr-only" data-testid="brand-name">{{ name }}</span>
    </template>
    <template v-else>
      <img v-if="brand.logo" :src="brand.logo" :alt="name" :width="size" :height="size" class="rounded-xl object-contain shrink-0" :style="{ width: size + 'px', height: size + 'px' }" data-testid="brand-logo" />
      <span class="font-bold tracking-tight break-words" :style="{ fontSize: size * 0.5 + 'px' }" data-testid="brand-name">{{ name }}</span>
    </template>
  </span>
</template>
