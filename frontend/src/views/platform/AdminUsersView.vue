<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useAuthStore, type Profile } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { listUsers, updateUser } from '@/api/platformAdmin'

const pt = usePt()
const auth = useAuthStore()

const users = ref<Profile[]>([])
const filter = ref<'pending' | 'all'>('pending')
const q = ref('')
const role = ref('')
const page = ref(0)
const PAGE = 25
const hasMore = ref(false)
const loading = ref(false)
const error = ref('')
const notice = ref('')

const ROLES = ['student', 'teacher', 'moderator', 'institution_admin', 'admin'] as const
const roleKey: Record<string, PlatformKey> = {
  student: 'roleStudent', teacher: 'roleTeacher', moderator: 'roleModerator',
  institution_admin: 'roleInstitutionAdmin', admin: 'roleAdmin',
}
const statusKey: Record<string, PlatformKey> = {
  active: 'statusActive', pending: 'statusPending', rejected: 'statusRejected', suspended: 'statusSuspended',
}
const typeKey: Record<string, PlatformKey> = { school: 'typeSchool', institute: 'typeInstitute', university: 'typeUniversity' }

async function load() {
  loading.value = true
  error.value = ''
  try {
    // Ask for one extra row to know whether a next page exists (no separate count request).
    const rows = await listUsers({
      status: filter.value === 'pending' ? 'pending' : undefined,
      role: role.value || undefined,
      q: q.value.trim() || undefined,
      limit: PAGE + 1,
      offset: page.value * PAGE,
    })
    hasMore.value = rows.length > PAGE
    users.value = rows.slice(0, PAGE)
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}

function search() {
  page.value = 0
  load()
}
function go(delta: number) {
  page.value += delta
  load()
}

async function apply(u: Profile, patch: Parameters<typeof updateUser>[1]) {
  error.value = ''
  notice.value = ''
  try {
    await updateUser(u.id, patch)
    if (patch.password) notice.value = pt('passwordChanged')
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

function withReason(u: Profile, status: 'rejected' | 'suspended') {
  const reason = window.prompt(pt('reasonPrompt'))
  if (reason === null) return
  apply(u, { status, status_reason: reason })
}

function resetPassword(u: Profile) {
  const p = window.prompt(pt('newPasswordPrompt'))
  if (p) apply(u, { password: p })
}

onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('adminUsers') }}</h1>

    <div class="flex gap-2 mb-3">
      <button :class="filter === 'pending' ? 'btn-filled' : 'btn-outlined'" @click="filter = 'pending'; search()">{{ pt('pendingFilter') }}</button>
      <button :class="filter === 'all' ? 'btn-filled' : 'btn-outlined'" @click="filter = 'all'; search()">{{ pt('all') }}</button>
    </div>
    <form class="mb-4 flex gap-2" @submit.prevent="search">
      <input v-model="q" type="search" :placeholder="pt('search')" class="input-outlined flex-1 min-w-0" data-testid="users-search" />
      <select v-model="role" class="input-outlined" :aria-label="pt('accountRole')" data-testid="users-role-filter" @change="search">
        <option value="">{{ pt('allRoles') }}</option>
        <option v-for="r in ROLES" :key="r" :value="r">{{ pt(roleKey[r]!) }}</option>
      </select>
    </form>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="notice" class="text-body-sm mb-3" role="status">{{ notice }}</p>
    <p v-if="!loading && !users.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>

    <ul class="space-y-3">
      <li v-for="u in users" :key="u.id" class="card-filled p-4 space-y-3">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="font-bold truncate">{{ u.full_name }}</div>
            <div class="text-body-sm truncate" dir="ltr" style="color: rgb(var(--md-on-surface-variant))">{{ u.email }}</div>
            <div class="text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">
              {{ pt(roleKey[u.role]!) }}<template v-if="u.institution_type"> · {{ pt(typeKey[u.institution_type]!) }}</template>
            </div>
            <div v-if="u.status_reason" class="text-body-sm mt-1">{{ pt('reason') }}: {{ u.status_reason }}</div>
          </div>
          <span class="shrink-0 px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[u.status]!) }}</span>
        </div>

        <div class="flex flex-wrap gap-2 items-center">
          <template v-if="u.id !== auth.profile?.id">
            <button v-if="u.status === 'pending'" class="btn-filled" @click="apply(u, { status: 'active' })">{{ pt('approve') }}</button>
            <button v-if="u.status === 'pending'" class="btn-outlined" @click="withReason(u, 'rejected')">{{ pt('reject') }}</button>
            <button v-if="u.status === 'active'" class="btn-outlined" @click="withReason(u, 'suspended')">{{ pt('suspend') }}</button>
            <button v-if="u.status === 'rejected' || u.status === 'suspended'" class="btn-tonal" @click="apply(u, { status: 'active' })">{{ pt('reactivate') }}</button>
            <select
              :value="u.role"
              class="input-outlined !py-2"
              :aria-label="pt('accountRole')"
              @change="apply(u, { role: ($event.target as HTMLSelectElement).value })"
            >
              <option v-for="r in ROLES" :key="r" :value="r">{{ pt(roleKey[r]!) }}</option>
            </select>
          </template>
          <button class="btn-text" @click="resetPassword(u)">{{ pt('resetPassword') }}</button>
        </div>
      </li>
    </ul>

    <nav v-if="page > 0 || hasMore" class="flex items-center justify-between gap-3 mt-4" :aria-label="pt('pageLabel')" data-testid="users-pager">
      <button class="btn-outlined" :disabled="page === 0 || loading" data-testid="users-prev" @click="go(-1)">{{ pt('prevPage') }}</button>
      <span class="text-body-sm">{{ pt('pageLabel') }} <span dir="ltr" class="inline-block">{{ page + 1 }}</span></span>
      <button class="btn-outlined" :disabled="!hasMore || loading" data-testid="users-next" @click="go(1)">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
