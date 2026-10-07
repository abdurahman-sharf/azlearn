<script setup lang="ts">
import { onMounted } from 'vue'
import { usePt } from '@/i18n/platform'
import { useBrandingStore } from '@/stores/branding'
import BrandMark from '@/components/platform/BrandMark.vue'
import LegalLinks from '@/components/platform/LegalLinks.vue'
import { BuildingLibraryIcon, ClipboardDocumentCheckIcon, UserPlusIcon } from '@heroicons/vue/24/outline'

const pt = usePt()
const brand = useBrandingStore()
onMounted(() => brand.load()) // pick up identity/legal changes made since the app was opened
const steps = [
  { icon: UserPlusIcon, title: 'landingStep1Title', desc: 'landingStep1Desc' },
  { icon: BuildingLibraryIcon, title: 'landingStep2Title', desc: 'landingStep2Desc' },
  { icon: ClipboardDocumentCheckIcon, title: 'landingStep3Title', desc: 'landingStep3Desc' },
] as const
</script>

<template>
  <div class="max-w-3xl mx-auto pb-10">
    <section class="text-center py-8 sm:py-14 space-y-5" data-testid="landing-hero">
      <h1 class="flex justify-center" data-testid="landing-title"><BrandMark :size="72" /></h1>
      <p class="text-title-md font-semibold" style="color: rgb(var(--md-on-surface-variant))" data-testid="landing-motto">{{ pt('landingMotto') }}</p>
      <p class="text-body-lg max-w-xl mx-auto" style="color: rgb(var(--md-on-surface-variant))">{{ pt('landingTagline') }}</p>
      <p class="text-label-lg font-semibold" style="color: rgb(var(--md-on-primary-container))">{{ pt('landingInstitutions') }}</p>
      <div class="flex flex-wrap items-center justify-center gap-3 pt-2">
        <router-link to="/auth/register" class="btn-filled" data-testid="cta-register">{{ pt('landingStart') }} — {{ pt('register') }}</router-link>
        <router-link to="/auth/login" class="btn-teal" data-testid="cta-login">{{ pt('login') }}</router-link>
      </div>
    </section>

    <section class="space-y-4">
      <h2 class="text-title-lg font-bold text-center">{{ pt('landingHowTitle') }}</h2>
      <ol class="grid gap-3 sm:grid-cols-3">
        <li v-for="(s, i) in steps" :key="s.title" class="card-filled p-4 space-y-2">
          <div class="flex items-center gap-3">
            <span class="w-10 h-10 rounded-2xl flex items-center justify-center shrink-0" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))">
              <component :is="s.icon" class="w-5 h-5" aria-hidden="true" />
            </span>
            <span class="font-bold"><span dir="ltr" class="inline-block">{{ i + 1 }}.</span> {{ pt(s.title) }}</span>
          </div>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(s.desc) }}</p>
        </li>
      </ol>
    </section>

    <p class="text-center mt-8">
      <router-link to="/generate" class="underline text-body-sm">{{ pt('landingTry') }}</router-link>
    </p>
    <div class="mt-6"><LegalLinks /></div>
  </div>
</template>
