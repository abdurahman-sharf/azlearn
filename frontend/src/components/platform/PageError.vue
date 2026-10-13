<script setup lang="ts">
import { usePt } from '@/i18n/platform'

// The whole page when its data could not be loaded (a view that renders nothing else): a heading, the reason and a way
// out, so a screen-reader or keyboard user never lands on a page that is only a bare alert.
withDefaults(defineProps<{ message: string; backTo?: string }>(), { backTo: '/platform' })
const pt = usePt()
</script>

<template>
  <div class="max-w-3xl mx-auto space-y-3" data-testid="page-error">
    <router-link :to="backTo" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('loadErrorTitle') }}</h1>
    <p role="alert" style="color: rgb(var(--md-error))">{{ message }}</p>
  </div>
</template>
