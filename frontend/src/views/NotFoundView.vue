<script setup lang="ts">
import { computed } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { platformEnabled } from '@/lib/platformApi'
import { usePt } from '@/i18n/platform'

// Any address the app does not know (a mistyped or outdated link): say so and offer the way home instead of an empty page.
const pt = usePt()
const auth = useAuthStore()
const home = computed(() => (!platformEnabled ? '/practice' : auth.isLoggedIn ? '/platform' : '/'))
</script>

<template>
  <div class="max-w-xl mx-auto py-10 text-center space-y-4" data-testid="not-found">
    <p class="text-display-md font-bold" aria-hidden="true" style="color: rgb(var(--md-primary))"><span dir="ltr" class="inline-block">404</span></p>
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('nfTitle') }}</h1>
    <p class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('nfBody') }}</p>
    <router-link :to="home" class="btn-filled inline-flex" data-testid="not-found-home">{{ pt('nfHome') }}</router-link>
  </div>
</template>
