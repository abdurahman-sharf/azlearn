<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useAdminStats } from '@/lib/adminStats'

const pt = usePt()
const auth = useAuthStore()
const { stats, refresh } = useAdminStats()
onMounted(refresh)

const totalUsers = computed(() => Object.values(stats.value?.users ?? {}).reduce((a, b) => a + b, 0))
// Things that wait on the admin, each linking to the screen that resolves it.
type Todo = { to: string; label: PlatformKey; n: number; testid: string }
const todo = computed<Todo[]>(() => ([
  { to: '/platform/admin/users', label: 'todoPendingTeachers', n: stats.value?.pending_teachers ?? 0, testid: 'todo-teachers' },
  { to: '/platform/admin/teaching', label: 'todoPendingTeaching', n: stats.value?.pending_teaching ?? 0, testid: 'todo-teaching' },
  { to: '/platform/admin/reports', label: 'todoOpenReports', n: stats.value?.open_reports ?? 0, testid: 'todo-reports' },
] as Todo[]).filter((t) => t.n > 0))
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('adminOverview') }}</h1>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">{{ pt('welcome') }} {{ auth.profile?.full_name }}</p>

    <section v-if="stats" class="mb-6">
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-2" data-testid="stats">
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statUsers') }}</div><div class="text-title-lg font-bold" data-testid="stat-users">{{ totalUsers }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statPendingTeachers') }}</div><div class="text-title-lg font-bold" data-testid="stat-pending">{{ stats.pending_teachers }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statOpenReports') }}</div><div class="text-title-lg font-bold" data-testid="stat-reports">{{ stats.open_reports }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statSignups') }}</div><div class="text-title-lg font-bold">{{ stats.signups_7d }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statContent') }}</div><div class="text-title-lg font-bold">{{ stats.content.posts + stats.content.courses + stats.content.assessments }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statAttempts') }}</div><div class="text-title-lg font-bold">{{ stats.attempts_submitted }}</div></div>
      </div>
    </section>

    <section class="mb-6">
      <h2 class="text-title-md font-bold mb-3">{{ pt('adminTodo') }}</h2>
      <p v-if="stats && !todo.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="todo-empty">{{ pt('adminTodoEmpty') }}</p>
      <ul class="space-y-2">
        <li v-for="t in todo" :key="t.to">
          <router-link :to="t.to" :data-testid="t.testid" class="card-filled flex items-center justify-between gap-3 p-4 no-underline">
            <span class="font-bold">{{ pt(t.label) }}</span>
            <span class="px-3 rounded-full text-sm font-bold" style="background-color: rgb(var(--md-primary)); color: rgb(var(--md-on-primary))"><span dir="ltr" class="inline-block">{{ t.n }}</span></span>
          </router-link>
        </li>
      </ul>
    </section>
  </div>
</template>
