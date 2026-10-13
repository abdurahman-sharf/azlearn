<script setup lang="ts">
import { computed, onMounted, ref, type Component } from 'vue'
import {
  ArchiveBoxIcon, BookOpenIcon, BuildingLibraryIcon, ClipboardDocumentCheckIcon, ClipboardDocumentListIcon, Cog6ToothIcon,
  PlusCircleIcon, QuestionMarkCircleIcon, UsersIcon,
} from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useAdminStats } from '@/lib/adminStats'
import { institutionStats, type InstitutionStat } from '@/api/platformStats'

const pt = usePt()
const auth = useAuthStore()
const { stats, refresh } = useAdminStats()
const perInstitution = ref<InstitutionStat[] | null>(null)
onMounted(() => {
  refresh()
  institutionStats().then((s) => { perInstitution.value = s }).catch(() => { /* the cards below still work */ })
})

const totalUsers = computed(() => Object.values(stats.value?.users ?? {}).reduce((a, b) => a + b, 0))
const sum = (k: 'subjects' | 'attempts' | 'questions') => (perInstitution.value ?? []).reduce((a, i) => a + i[k], 0)

// The headline numbers of the platform.
interface Kpi { label: PlatformKey; value: number | null; icon: Component; testid: string }
const kpis = computed<Kpi[]>(() => [
  { label: 'statUsers', value: stats.value ? totalUsers.value : null, icon: UsersIcon, testid: 'stat-users' },
  { label: 'statSubjects', value: perInstitution.value ? sum('subjects') : null, icon: BookOpenIcon, testid: 'kpi-subjects' },
  { label: 'stcQuestions', value: perInstitution.value ? sum('questions') : null, icon: QuestionMarkCircleIcon, testid: 'kpi-questions' },
  { label: 'stcAttempts', value: perInstitution.value ? sum('attempts') : null, icon: ClipboardDocumentCheckIcon, testid: 'kpi-attempts' },
])

// Things that wait on the admin, each linking to the screen that resolves it.
type Todo = { to: string; label: PlatformKey; n: number; testid: string }
const todo = computed<Todo[]>(() => ([
  { to: '/platform/admin/users', label: 'todoPendingTeachers', n: stats.value?.pending_teachers ?? 0, testid: 'todo-teachers' },
  { to: '/platform/admin/teaching', label: 'todoPendingTeaching', n: stats.value?.pending_teaching ?? 0, testid: 'todo-teaching' },
  { to: '/platform/admin/reports', label: 'todoOpenReports', n: stats.value?.open_reports ?? 0, testid: 'todo-reports' },
] as Todo[]).filter((t) => t.n > 0))

const quick: { to: string; label: PlatformKey; icon: Component; testid: string }[] = [
  { to: '/platform/admin/exams/new', label: 'exNew', icon: PlusCircleIcon, testid: 'quick-exam' },
  { to: '/platform/admin/exams', label: 'exTitle', icon: ClipboardDocumentListIcon, testid: 'quick-exams' },
  { to: '/platform/admin/bank', label: 'bankTitle', icon: ArchiveBoxIcon, testid: 'quick-bank' },
  { to: '/platform/admin/institutions', label: 'adminInstitutions', icon: BuildingLibraryIcon, testid: 'quick-institutions' },
  { to: '/platform/admin/users', label: 'adminUsers', icon: UsersIcon, testid: 'quick-users' },
  { to: '/platform/admin/settings', label: 'settingsTitle', icon: Cog6ToothIcon, testid: 'quick-settings' },
]
</script>

<template>
  <div class="max-w-5xl mx-auto pb-8" data-testid="admin-overview">
    <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('adminOverview') }}</h1>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">{{ pt('welcome') }} {{ auth.profile?.full_name }}</p>

    <section class="mb-6" :aria-label="pt('admKpiTitle')">
      <ul class="grid grid-cols-2 lg:grid-cols-4 gap-3" data-testid="kpis">
        <li v-for="k in kpis" :key="k.testid" class="card-filled p-4 flex items-center gap-3">
          <span class="w-12 h-12 shrink-0 rounded-2xl flex items-center justify-center" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" aria-hidden="true">
            <component :is="k.icon" class="w-6 h-6" />
          </span>
          <span class="min-w-0">
            <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(k.label) }}</span>
            <b class="block text-title-lg" dir="ltr" :data-testid="k.testid">{{ k.value ?? '—' }}</b>
          </span>
        </li>
      </ul>
    </section>

    <section v-if="stats" class="mb-6">
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-2" data-testid="stats">
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statPendingTeachers') }}</div><div class="text-title-lg font-bold" data-testid="stat-pending">{{ stats.pending_teachers }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statOpenReports') }}</div><div class="text-title-lg font-bold" data-testid="stat-reports">{{ stats.open_reports }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statSignups') }}</div><div class="text-title-lg font-bold">{{ stats.signups_7d }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statContent') }}</div><div class="text-title-lg font-bold">{{ stats.content.posts + stats.content.courses + stats.content.assessments }}</div></div>
      </div>
    </section>

    <section class="mb-6">
      <h2 class="text-title-md font-bold mb-3">{{ pt('adminTodo') }}</h2>
      <p v-if="stats && !todo.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="todo-empty">{{ pt('adminTodoEmpty') }}</p>
      <ul class="space-y-2">
        <li v-for="t in todo" :key="t.to">
          <router-link :to="t.to" :data-testid="t.testid" class="card-filled flex items-center justify-between gap-3 p-4 no-underline">
            <span class="font-bold">{{ pt(t.label) }}</span>
            <span class="px-3 rounded-full text-sm font-bold" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))"><span dir="ltr" class="inline-block">{{ t.n }}</span></span>
          </router-link>
        </li>
      </ul>
    </section>

    <section class="mb-2">
      <h2 class="text-title-md font-bold mb-3">{{ pt('admQuickTitle') }}</h2>
      <ul class="grid grid-cols-2 sm:grid-cols-3 gap-3" data-testid="quick-actions">
        <li v-for="q in quick" :key="q.to">
          <router-link :to="q.to" :data-testid="q.testid" class="card-filled flex items-center gap-3 p-4 no-underline font-bold h-full">
            <component :is="q.icon" class="w-6 h-6 shrink-0" style="color: rgb(var(--md-primary))" aria-hidden="true" />
            <span>{{ pt(q.label) }}</span>
          </router-link>
        </li>
      </ul>
    </section>
  </div>
</template>
