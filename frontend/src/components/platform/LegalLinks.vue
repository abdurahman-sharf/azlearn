<script setup lang="ts">
import { useBrandingStore } from '@/stores/branding'
import { usePt } from '@/i18n/platform'

const brand = useBrandingStore()
const pt = usePt()
</script>

<template>
  <nav v-if="brand.legal.terms || brand.legal.privacy" :aria-label="pt('legalNav')" class="flex flex-wrap justify-center gap-x-4 gap-y-1 text-body-sm" data-testid="legal-links">
    <router-link v-if="brand.legal.terms" to="/legal/terms" class="underline">{{ pt('termsPage') }}</router-link>
    <router-link v-if="brand.legal.privacy" to="/legal/privacy" class="underline">{{ pt('privacyPage') }}</router-link>
  </nav>
</template>
