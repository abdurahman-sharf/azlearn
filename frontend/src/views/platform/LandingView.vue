<script setup lang="ts">
import { onMounted, type Component } from 'vue'
import {
  AcademicCapIcon, BellAlertIcon, BuildingLibraryIcon, ChartBarIcon, CheckCircleIcon, ClipboardDocumentCheckIcon, ClockIcon,
  PresentationChartLineIcon, SparklesIcon, UserGroupIcon, UserPlusIcon, UsersIcon,
} from '@heroicons/vue/24/outline'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useBrandingStore } from '@/stores/branding'
import BrandMark from '@/components/platform/BrandMark.vue'
import LegalLinks from '@/components/platform/LegalLinks.vue'
import LiveStats from '@/components/landing/LiveStats.vue'

const pt = usePt()
const brand = useBrandingStore()
onMounted(() => brand.load()) // pick up identity/legal changes made since the app was opened

// Only things the platform really does.
interface Feature { icon: Component; title: PlatformKey; desc: PlatformKey }
const features: Feature[] = [
  { icon: ClockIcon, title: 'lpF1Title', desc: 'lpF1Desc' },
  { icon: ChartBarIcon, title: 'lpF2Title', desc: 'lpF2Desc' },
  { icon: PresentationChartLineIcon, title: 'lpF3Title', desc: 'lpF3Desc' },
  { icon: BuildingLibraryIcon, title: 'lpF4Title', desc: 'lpF4Desc' },
  { icon: BellAlertIcon, title: 'lpF5Title', desc: 'lpF5Desc' },
  { icon: SparklesIcon, title: 'lpF6Title', desc: 'lpF6Desc' },
]
interface Audience { icon: Component; title: PlatformKey; points: PlatformKey[]; testid: string }
const audiences: Audience[] = [
  { icon: AcademicCapIcon, title: 'lpAudSTitle', points: ['lpAudS1', 'lpAudS2', 'lpAudS3'], testid: 'aud-students' },
  { icon: UsersIcon, title: 'lpAudTTitle', points: ['lpAudT1', 'lpAudT2', 'lpAudT3'], testid: 'aud-teachers' },
  { icon: BuildingLibraryIcon, title: 'lpAudITitle', points: ['lpAudI1', 'lpAudI2', 'lpAudI3'], testid: 'aud-institutions' },
]
const steps = [
  { icon: UserPlusIcon, title: 'landingStep1Title', desc: 'landingStep1Desc' },
  { icon: BuildingLibraryIcon, title: 'landingStep2Title', desc: 'landingStep2Desc' },
  { icon: ClipboardDocumentCheckIcon, title: 'landingStep3Title', desc: 'landingStep3Desc' },
] as const
</script>

<template>
  <div class="max-w-5xl mx-auto pb-10 space-y-16 sm:space-y-20">
    <!-- hero -->
    <section class="text-center pt-8 sm:pt-14 space-y-5" data-testid="landing-hero">
      <h1 class="flex justify-center" data-testid="landing-title"><BrandMark :size="72" /></h1>
      <p class="text-title-lg font-bold" style="color: rgb(var(--md-on-surface))" data-testid="landing-motto">{{ pt('landingMotto') }}</p>
      <p class="text-body-lg max-w-xl mx-auto" style="color: rgb(var(--md-on-surface-variant))">{{ pt('landingTagline') }}</p>
      <p class="text-label-lg font-semibold" style="color: rgb(var(--md-on-primary-container))">{{ pt('landingInstitutions') }}</p>
      <div class="flex flex-wrap items-center justify-center gap-3 pt-2">
        <router-link to="/auth/register" class="btn-filled" data-testid="cta-register">{{ pt('landingStart') }} — {{ pt('register') }}</router-link>
        <router-link to="/auth/login" class="btn-teal" data-testid="cta-login">{{ pt('login') }}</router-link>
      </div>
    </section>

    <LiveStats />

    <!-- features -->
    <section id="landing-features" class="scroll-mt-24 space-y-6" aria-labelledby="lp-features-title" data-testid="landing-features">
      <div class="text-center space-y-2">
        <h2 id="lp-features-title" tabindex="-1" class="text-display-sm font-bold tracking-tight focus:outline-none">{{ pt('lpFeaturesTitle') }}</h2>
        <p class="text-body-lg max-w-2xl mx-auto" style="color: rgb(var(--md-on-surface-variant))">{{ pt('lpFeaturesSub') }}</p>
      </div>
      <ul class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <li v-for="f in features" :key="f.title" class="card-filled p-5 space-y-3" data-testid="feature-card">
          <span class="w-12 h-12 rounded-2xl flex items-center justify-center" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" aria-hidden="true">
            <component :is="f.icon" class="w-6 h-6" />
          </span>
          <h3 class="text-title-md font-bold">{{ pt(f.title) }}</h3>
          <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt(f.desc) }}</p>
        </li>
      </ul>
    </section>

    <!-- who it is for -->
    <section id="landing-audience" class="scroll-mt-24 space-y-6" aria-labelledby="lp-audience-title" data-testid="landing-audience">
      <div class="text-center space-y-2">
        <h2 id="lp-audience-title" tabindex="-1" class="text-display-sm font-bold tracking-tight focus:outline-none">{{ pt('lpAudienceTitle') }}</h2>
        <p class="text-body-lg max-w-2xl mx-auto" style="color: rgb(var(--md-on-surface-variant))">{{ pt('lpAudienceSub') }}</p>
      </div>
      <ul class="grid gap-4 md:grid-cols-3">
        <li v-for="a in audiences" :key="a.title" class="card-elevated p-5 space-y-4" :data-testid="a.testid">
          <div class="flex items-center gap-3">
            <span class="w-12 h-12 rounded-2xl flex items-center justify-center" style="background-color: rgb(var(--azl-deep-blue)); color: #fff" aria-hidden="true">
              <component :is="a.icon" class="w-6 h-6" />
            </span>
            <h3 class="text-title-lg font-bold">{{ pt(a.title) }}</h3>
          </div>
          <ul class="space-y-2">
            <li v-for="p in a.points" :key="p" class="flex items-start gap-2 text-body-md">
              <CheckCircleIcon class="w-5 h-5 mt-0.5 shrink-0" style="color: rgb(var(--azl-success-text))" aria-hidden="true" />
              <span>{{ pt(p) }}</span>
            </li>
          </ul>
        </li>
      </ul>
    </section>

    <!-- how it works -->
    <section id="landing-how" class="scroll-mt-24 space-y-5" aria-labelledby="lp-how-title" data-testid="landing-how">
      <h2 id="lp-how-title" tabindex="-1" class="text-display-sm font-bold tracking-tight text-center focus:outline-none">{{ pt('landingHowTitle') }}</h2>
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

    <!-- final call -->
    <section class="rounded-3xl px-6 py-10 text-center space-y-4" style="background-color: rgb(var(--azl-deep-blue))" aria-labelledby="lp-cta-title" data-testid="landing-cta">
      <UserGroupIcon class="w-9 h-9 mx-auto" style="color: rgb(var(--azl-teal))" aria-hidden="true" />
      <h2 id="lp-cta-title" class="text-display-sm font-bold tracking-tight" style="color: #fff">{{ pt('lpCtaTitle') }}</h2>
      <p class="text-body-lg max-w-xl mx-auto" style="color: rgb(219 234 254)">{{ pt('lpCtaSub') }}</p>
      <div class="flex flex-wrap items-center justify-center gap-3 pt-1">
        <router-link to="/auth/register" class="btn-teal" data-testid="cta2-register">{{ pt('landingStart') }} — {{ pt('register') }}</router-link>
        <router-link to="/auth/login" class="no-underline font-bold px-5 py-2.5 rounded-full border-2" style="color: #fff; border-color: #fff" data-testid="cta2-login">{{ pt('login') }}</router-link>
      </div>
    </section>

    <LegalLinks />
  </div>
</template>
