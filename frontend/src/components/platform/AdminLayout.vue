<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useAdminStats } from '@/lib/adminStats'

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const { stats, refresh } = useAdminStats()

onMounted(refresh)
// Approving/rejecting something changes the badges; refresh when moving between sections.
watch(() => route.path, refresh)

interface Item { to: string; label: PlatformKey; badge?: number; exact?: boolean; testid: string }
const items = computed<Item[]>(() => [
  { to: '/platform/admin', label: 'adminOverview', exact: true, testid: 'nav-overview' },
  { to: '/platform/admin/exams', label: 'exTitle', testid: 'nav-exams' },
  { to: '/platform/grading', label: 'navGrading', badge: stats.value?.pending_grading, testid: 'nav-grading' },
  { to: '/platform/admin/bank', label: 'bankTitle', testid: 'nav-bank' },
  { to: '/platform/admin/users', label: 'adminUsers', badge: stats.value?.pending_teachers, testid: 'nav-users' },
  { to: '/platform/admin/teaching', label: 'adminTeaching', badge: stats.value?.pending_teaching, testid: 'nav-teaching' },
  { to: '/platform/admin/institutions', label: 'adminInstitutions', testid: 'nav-institutions' },
  { to: '/platform/admin/reports', label: 'adminReports', badge: stats.value?.open_reports, testid: 'nav-reports' },
  { to: '/platform/admin/audit', label: 'adminAudit', testid: 'nav-audit' },
  { to: '/platform/admin/settings', label: 'settingsTitle', testid: 'nav-settings' },
  { to: '/platform/admin/system', label: 'sysTitle', badge: stats.value?.system_warnings, testid: 'nav-system' },
])
const extra: { to: string; label: PlatformKey; testid: string }[] = [
  { to: '/platform/notifications', label: 'notifications', testid: 'nav-notifications' },
  { to: '/platform/account', label: 'accountSettings', testid: 'nav-account' },
]
const isActive = (i: { to: string; exact?: boolean }) => (i.exact ? route.path === i.to : route.path === i.to || route.path.startsWith(i.to + '/'))

async function logout() {
  await auth.signOut()
  router.replace('/mine')
}
</script>

<template>
  <div class="mx-auto w-full max-w-6xl flex flex-col md:flex-row gap-4 md:gap-8" data-testid="admin-shell">
    <aside class="md:w-60 shrink-0 md:sticky md:top-20 md:self-start">
      <nav :aria-label="pt('adminNav')" class="flex md:flex-col gap-1 overflow-x-auto md:overflow-visible pb-2 md:pb-0 -mx-1 px-1" data-testid="admin-nav">
        <router-link
          v-for="i in items"
          :key="i.to"
          :to="i.to"
          :data-testid="i.testid"
          :aria-current="isActive(i) ? 'page' : undefined"
          class="flex items-center justify-between gap-3 whitespace-nowrap rounded-full px-4 py-2.5 text-body-md font-semibold no-underline transition-colors"
          :style="isActive(i)
            ? { backgroundColor: 'rgb(var(--md-primary-container))', color: 'rgb(var(--md-on-primary-container))' }
            : { color: 'rgb(var(--md-on-surface))' }"
        >
          <span>{{ pt(i.label) }}</span>
          <span
            v-if="i.badge"
            class="min-w-[1.5rem] text-center rounded-full px-2 text-xs font-bold"
            :data-testid="i.testid + '-badge'"
            style="background-color: rgb(var(--md-primary)); color: rgb(var(--md-on-primary))"
          ><span dir="ltr" class="inline-block">{{ i.badge }}</span></span>
        </router-link>
        <span class="hidden md:block my-2 border-t" style="border-color: rgb(var(--md-outline-variant))" aria-hidden="true"></span>
        <router-link
          v-for="i in extra"
          :key="i.to"
          :to="i.to"
          :data-testid="i.testid"
          class="rounded-full px-4 py-2.5 text-body-md whitespace-nowrap no-underline"
          style="color: rgb(var(--md-on-surface-variant))"
        >{{ pt(i.label) }}</router-link>
        <button class="rounded-full px-4 py-2.5 text-body-md whitespace-nowrap text-start" style="color: rgb(var(--md-on-surface-variant))" data-testid="nav-logout" @click="logout">{{ pt('logout') }}</button>
      </nav>
    </aside>
    <main class="min-w-0 flex-1">
      <router-view />
    </main>
  </div>
</template>
