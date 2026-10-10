<script setup lang="ts">
import { ref, nextTick, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, formatNotification, type PlatformKey } from '@/i18n/platform'
import { notifications, markRead, type AppNotification, type NotificationGroup } from '@/api/platformEngage'
import { useAuthStore } from '@/stores/auth'
import { useTeacherStats } from '@/lib/teacherStats'
import { fillTemplate } from '@/utils/notificationText'

const pt = usePt()
const i18n = useI18nStore()
const router = useRouter()
const PAGE = 20
const GROUPS: { key: '' | NotificationGroup; label: PlatformKey }[] = [
  { key: '', label: 'notifGroupAll' },
  { key: 'exams', label: 'notifGroupExams' },
  { key: 'content', label: 'notifGroupContent' },
  { key: 'people', label: 'notifGroupPeople' },
  { key: 'account', label: 'notifGroupAccount' },
]

const items = ref<AppNotification[]>([])
// the user's TOTAL unread count (the server does not narrow it by the filters)
const unread = ref(0)
const next = ref<string | null>(null)
const group = ref<'' | NotificationGroup>('')
const unreadOnly = ref(false)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const list = ref<HTMLUListElement | null>(null)
// the teacher's sidebar shows the unread count: reading here must update it without waiting for the next page change
const auth = useAuthStore()
const { refresh: refreshBadges } = useTeacherStats()
const syncBadge = () => { if (auth.role === 'teacher') refreshBadges() }

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

// Answers to the latest request only: a slow page for an old filter must not land in the list of a newer one.
let latest = 0
/** `more` appends the page after `next`; otherwise the list starts over from the newest notification. */
async function load(more = false) {
  const mine = ++latest
  loading.value = true
  error.value = ''
  try {
    const r = await notifications({
      limit: PAGE, group: group.value || undefined, unread: unreadOnly.value,
      ...(more && next.value ? { cursor: next.value } : {}),
    })
    if (mine !== latest) return
    if (more) {
      const before = items.value.length
      const seen = new Set(items.value.map((n) => n.id))
      items.value = [...items.value, ...r.items.filter((n) => !seen.has(n.id))]
      unread.value = r.unread
      next.value = r.next ?? null
      await keepPlace(before)
      return
    }
    items.value = r.items
    unread.value = r.unread
    next.value = r.next ?? null
  } catch (e) {
    if (mine === latest) error.value = platformErrorMessage(pt, e)
  } finally {
    if (mine === latest) {
      loading.value = false
      loaded.value = true
    }
  }
}

/**
 * After "Load more": keyboard focus goes to the first row that was just added (the button the user pressed disappears on
 * the last page, and a focused element that vanishes sends the next Tab back to the top of a possibly very long list).
 * With no new row at all the list itself takes the focus.
 */
async function keepPlace(before: number) {
  await nextTick()
  const rows = list.value?.querySelectorAll<HTMLElement>('[data-testid="notif-row"] button')
  const target = rows && rows.length > before ? rows[before] : list.value
  target?.focus()
}

function loadMore() {
  // aria-disabled instead of disabled: a disabled button drops the keyboard focus while the page is on its way
  if (loading.value) return
  load(true)
}

function setGroup(g: '' | NotificationGroup) {
  if (group.value === g) return
  group.value = g
  load()
}
function toggleUnread() {
  unreadOnly.value = !unreadOnly.value
  load()
}

async function open(n: AppNotification) {
  if (!n.read) {
    n.read = true
    unread.value = Math.max(0, unread.value - 1)
    try {
      await markRead([n.id])
      syncBadge()
    } catch { /* opening the link matters more than the read flag; the next load shows the truth */ }
  }
  if (n.link && n.link.startsWith('/')) router.push(n.link)
}

async function readAll() {
  try {
    await markRead()
    await load()
    syncBadge()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(() => load())
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <div class="flex items-center gap-3 my-3">
      <h1 class="text-display-sm font-bold tracking-tight flex-1">{{ pt('notifications') }}</h1>
      <button v-if="unread" class="btn-outlined" data-testid="notif-mark-all" @click="readAll">{{ pt('markAllRead') }}</button>
    </div>

    <div class="flex items-center gap-2 flex-wrap mb-3">
      <div class="flex gap-2 flex-wrap" role="group" :aria-label="pt('notifGroupLabel')">
        <button v-for="g in GROUPS" :key="g.key" type="button" :class="group === g.key ? 'btn-filled' : 'btn-outlined'" :aria-pressed="group === g.key" :data-testid="`notif-group-${g.key || 'all'}`" @click="setGroup(g.key)">{{ pt(g.label) }}</button>
      </div>
      <label class="flex items-center gap-2 text-body-sm ms-auto"><input type="checkbox" :checked="unreadOnly" data-testid="notif-unread-only" @change="toggleUnread" /> {{ pt('notifUnreadOnly') }}</label>
    </div>

    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))" data-testid="notif-error">{{ error }}</p>
    <p v-if="loaded" class="sr-only" role="status" data-testid="notif-status">{{ fillTemplate(pt('notifShowing'), { n: items.length }) }}</p>
    <p v-if="loaded && !items.length && !error" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="notif-empty">{{ group || unreadOnly ? pt('notifNoMatch') : pt('noNotifications') }}</p>
    <ul ref="list" tabindex="-1" class="space-y-2 focus:outline-none" :aria-busy="loading" data-testid="notif-list">
      <li v-for="n in items" :key="n.id" data-testid="notif-row">
        <button class="w-full text-start card-filled p-3" :class="{ 'ring-2': !n.read }" @click="open(n)">
          <div class="break-words" :class="{ 'font-bold': !n.read }">
            <span class="sr-only">{{ n.read ? pt('notifRead') : pt('notifUnread') }}: </span>{{ formatNotification(pt, n.kind, n.data, fmt) }}
          </div>
          <div class="text-body-sm flex items-center gap-2 flex-wrap" style="color: rgb(var(--md-on-surface-variant))">
            <span v-if="!n.read" aria-hidden="true" class="text-xs font-bold px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-primary)); color: rgb(var(--md-on-primary))">{{ pt('notifNew') }}</span>
            <span>{{ fmt(n.created_at) }}</span>
          </div>
        </button>
      </li>
    </ul>
    <div v-if="next" class="flex justify-center mt-4">
      <button class="btn-outlined" :aria-disabled="loading" data-testid="notif-more" @click="loadMore">{{ pt('loadMore') }}</button>
    </div>
  </div>
</template>
