<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { useTheme } from '@/composables/useTheme'
import { platformEnabled } from '@/lib/platformApi'
import { useBrandingStore } from '@/stores/branding'
import { useAuthStore } from '@/stores/auth'
import { usePt } from '@/i18n/platform'
import BrandMark from '@/components/platform/BrandMark.vue'
import AdminSidebar from '@/components/platform/AdminSidebar.vue'
import LandingNav from './LandingNav.vue'
import { isTauri, isMacOS, isWindows, isLinux } from '@/utils/platform'
import TitleBar from './TitleBar.vue'
import CookieBanner from './CookieBanner.vue'
import UpdateDialog from './UpdateDialog.vue'
import LanguageDialog from './LanguageDialog.vue'
import {
  SparklesIcon,
  SunIcon,
  MoonIcon,
  ComputerDesktopIcon,
  AcademicCapIcon,
  MagnifyingGlassIcon,
  UserCircleIcon,
  LanguageIcon,
} from '@heroicons/vue/24/outline'

const router = useRouter()
const route = useRoute()
const i18n = useI18nStore()
const showLanguageDialog = ref(false)
const brand = useBrandingStore()
const auth = useAuthStore()
const pt = usePt()

// Admin frame: on every /platform page an admin opens, the admin navigation sits beside <main> (not inside it, so the
// skip link still bypasses it and there is a single main landmark). The router guards have awaited auth.init() by now.
const adminFrame = computed(() => platformEnabled && auth.role === 'admin' && /^\/platform(\/|$)/.test(route.path))
// Landing header: routes flagged `landingHeader` show the landing menu instead of the practice/generate/search links.
const landing = computed(() => platformEnabled && route.meta.landingHeader === true)
const mainEl = ref<HTMLElement | null>(null)
function focusMain() {
  mainEl.value?.focus()
  mainEl.value?.scrollIntoView({ block: 'start' })
}
// On web/Docker the header carries the platform's own identity (name + logo) and links to the landing page.
const homePath = platformEnabled ? '/' : '/practice'
const headerName = computed(() => (platformEnabled ? brand.name ?? pt('defaultPlatformName') : i18n.t('appName')))

const { theme, cycleTheme } = useTheme()
const isDesktopTauri = isTauri() && (isWindows() || isMacOS() || isLinux())

async function openGitHub() {
  const url = 'https://github.com/heshengtao/exameow'
  if (isTauri()) {
    const { openUrl } = await import('@tauri-apps/plugin-opener')
    await openUrl(url)
  } else {
    window.open(url, '_blank')
  }
}

const navItems = [
  { key: 'navPractice', path: '/practice', icon: AcademicCapIcon },
  { key: 'navGenerate', path: '/generate', icon: SparklesIcon },
  { key: 'navSearch', path: '/search', icon: MagnifyingGlassIcon },
  { key: 'navMine', path: '/mine', icon: UserCircleIcon },
]

function isNavActive(item: { path: string }): boolean {
  return route.path === item.path || route.path.startsWith(item.path + '/')
}

const currentNavIndex = computed(() => navItems.findIndex(item => isNavActive(item)))

// Measure active button offset & width for dynamic desktop sliding pill
const desktopNavButtons = ref<(HTMLElement | null)[]>([])
const pillStyle = ref<{ left: string; width: string }>({ left: '0px', width: '0px' })

function updatePillPosition() {
  nextTick(() => {
    const idx = currentNavIndex.value
    if (idx >= 0 && desktopNavButtons.value[idx]) {
      const btn = desktopNavButtons.value[idx]!
      pillStyle.value = {
        left: `${btn.offsetLeft}px`,
        width: `${btn.offsetWidth}px`,
      }
    }
  })
}

watch([currentNavIndex, () => i18n.locale], () => {
  updatePillPosition()
})

onMounted(() => {
  updatePillPosition()
  window.addEventListener('resize', updatePillPosition)
})

onUnmounted(() => {
  window.removeEventListener('resize', updatePillPosition)
})

const headerStyle = {
  backgroundColor: 'rgba(var(--md-surface) / 0.86)',
  backdropFilter: 'blur(16px)',
  WebkitBackdropFilter: 'blur(16px)',
  borderBottom: '1px solid rgb(var(--md-outline-variant) / 0.3)',
} as any
</script>

<template>
  <div class="min-h-screen flex flex-col" :style="{ backgroundColor: 'rgb(var(--md-surface))' }">
    <!-- ====== Desktop TitleBar (Tauri only) ====== -->
    <TitleBar v-if="isDesktopTauri" />

    <!-- Bypass block (WCAG 2.4.1): first tab stop jumps over the navigation. Not an href="#…": the hash is the router's. -->
    <a class="skip-link" href="#main-content" data-testid="skip-link" @click.prevent="focusMain">{{ pt('skipToContent') }}</a>

    <!-- ====== Top App Bar ====== -->
    <header
      class="sticky z-30 select-none"
      :class="isDesktopTauri ? 'top-[38px]' : 'top-0 safe-top'"
      :style="headerStyle"
    >
      <div class="mx-auto w-full max-w-[90rem] flex items-center h-14 sm:h-16 gap-2 sm:gap-3 px-3 sm:px-6">
        <!-- Logo (browser / mobile only — desktop shows it in TitleBar) -->
        <router-link
          v-if="!isDesktopTauri"
          :to="homePath"
          class="flex items-center gap-3 shrink-0 no-underline group"
        >
          <template v-if="platformEnabled"><BrandMark :size="38" data-testid="header-brand" /></template>
          <template v-else>
            <img src="/logo.png" alt="Exameow" class="w-[38px] h-[38px] rounded-xl object-contain shrink-0 transition-transform duration-300 group-hover:scale-105" data-testid="header-logo" />
            <div class="hidden sm:block">
              <div class="text-title-md leading-tight font-bold tracking-tight" style="color: rgb(var(--md-on-surface))" data-testid="header-name">{{ headerName }}</div>
              <div class="text-label-sm" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('appSubtitle') }}</div>
            </div>
          </template>
        </router-link>

        <LandingNav v-if="landing" mode="top" />

        <!-- Desktop Nav — Pixel Segmented sliding pill navigation -->
        <div v-else class="hidden sm:flex items-center" :class="isDesktopTauri ? '' : 'ml-6'">
          <nav
            :aria-label="headerName"
            class="relative flex items-center p-1 rounded-full gap-0.5 shadow-sm"
            style="background-color: rgb(var(--md-surface-container-high))"
          >
            <!-- Sliding spring active background pill -->
            <div
              v-if="currentNavIndex >= 0"
              class="absolute top-1 bottom-1 rounded-full transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)] pointer-events-none"
              style="background-color: rgb(var(--md-secondary-container))"
              :style="pillStyle"
            />
            <button
              v-for="(item, idx) in navItems"
              :key="item.path"
              :ref="(el) => { if (el) desktopNavButtons[idx] = el as HTMLElement }"
              class="relative z-10 flex items-center justify-center gap-2 px-4 h-9 rounded-full text-sm font-medium transition-colors duration-200 cursor-pointer select-none"
              :style="{
                color: isNavActive(item) ? 'rgb(var(--md-on-secondary-container))' : 'rgb(var(--md-on-surface-variant))'
              }"
              @click="router.push(item.path)"
            >
              <component :is="item.icon" class="w-4 h-4 transition-transform duration-200" :class="{ 'scale-110': isNavActive(item) }" />
              <span>{{ i18n.t(item.key as any) }}</span>
            </button>
          </nav>
        </div>

        <!-- Spacer -->
        <div class="flex-1 self-stretch cursor-default" />

        <!-- Actions -->
        <div class="flex items-center gap-1.5">
          <template v-if="landing">
            <router-link to="/auth/login" class="btn-text hidden sm:inline-flex" data-testid="header-login">{{ pt('login') }}</router-link>
            <router-link to="/auth/register" class="btn-filled !px-4 !py-2 text-sm" data-testid="header-register">{{ pt('register') }}</router-link>
          </template>
          <button
            v-if="!landing"
            class="btn-icon"
            @click="openGitHub"
            title="GitHub"
            :style="{ color: 'rgb(var(--md-on-surface-variant))' }"
          >
            <svg class="w-5 h-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z" />
            </svg>
          </button>
          <button
            class="btn-icon"
            @click="showLanguageDialog = true"
            title="Language / 语言"
            :style="{ color: 'rgb(var(--md-on-surface-variant))' }"
          >
            <LanguageIcon class="w-5 h-5" />
          </button>
          <button class="btn-icon" @click="cycleTheme" :title="theme">
            <ComputerDesktopIcon v-if="theme === 'system'" class="w-5 h-5" />
            <SunIcon v-else-if="theme === 'light'" class="w-5 h-5" />
            <MoonIcon v-else class="w-5 h-5" />
          </button>
        </div>

      </div>
    </header>

    <!-- ====== Main Content ====== -->
    <div
      class="flex-1 mx-auto w-full px-3 sm:px-6 py-4 sm:py-7"
      :class="adminFrame ? 'max-w-[90rem] flex flex-col md:flex-row md:items-start gap-3 md:gap-8' : 'max-w-5xl xl:max-w-6xl'"
      :data-testid="adminFrame ? 'admin-shell' : undefined"
    >
      <AdminSidebar v-if="adminFrame" />
      <main id="main-content" ref="mainEl" tabindex="-1" class="focus:outline-none" :class="adminFrame ? 'min-w-0 w-full md:flex-1' : ''">
        <router-view v-slot="{ Component }">
          <!-- no page animation inside the admin frame: the sidebar stays put and every click is instant -->
          <transition :name="adminFrame ? 'none' : 'slide-up'" mode="out-in">
            <component :is="Component" />
          </transition>
        </router-view>
      </main>
    </div>

    <!-- ====== Bottom Navigation Bar (Mobile Pixel M3) ====== -->
    <LandingNav v-if="landing" mode="bottom" />
    <nav
      v-else
      :aria-label="headerName"
      class="sm:hidden sticky bottom-0 z-30 safe-bottom"
      :style="{
        backgroundColor: 'rgba(var(--md-surface-container-lowest) / 0.92)',
        backdropFilter: 'blur(20px)',
        WebkitBackdropFilter: 'blur(20px)',
        borderTop: '1px solid rgb(var(--md-outline-variant) / 0.25)',
      }"
    >
      <div class="flex items-center justify-around h-16 px-3">
        <div class="relative flex items-center justify-around w-full">
          <!-- Active spring pill indicator -->
          <div
            v-if="currentNavIndex >= 0"
            class="absolute top-1/2 -translate-y-1/2 h-8 rounded-full transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)] pointer-events-none"
            style="background-color: rgb(var(--md-secondary-container))"
            :style="{
              width: `calc(100% / ${navItems.length})`,
              left: i18n.locale === 'ar'
                ? `calc(${(navItems.length - 1 - currentNavIndex)} * 100% / ${navItems.length})`
                : `calc(${currentNavIndex} * 100% / ${navItems.length})`
            }"
          />

          <button
            v-for="item in navItems"
            :key="item.path"
            class="relative z-10 flex flex-col items-center justify-center gap-0.5 py-1 cursor-pointer select-none"
            :style="{
              width: `calc(100% / ${navItems.length})`,
              color: isNavActive(item) ? 'rgb(var(--md-on-secondary-container))' : 'rgb(var(--md-on-surface-variant))',
            }"
            @click="router.push(item.path)"
          >
            <component
              :is="item.icon"
              class="w-5 h-5 transition-transform duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
              :style="{ transform: isNavActive(item) ? 'scale(1.15) translateY(-1px)' : 'scale(1)' }"
            />
            <span class="text-[11px] font-semibold leading-tight tracking-tight">{{ i18n.t(item.key as any) }}</span>
          </button>
        </div>
      </div>
    </nav>
    <CookieBanner />
    <UpdateDialog />
    <LanguageDialog v-if="showLanguageDialog" @close="showLanguageDialog = false" />
  </div>
</template>

<style>
</style>
