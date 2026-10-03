<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, formatNotification } from '@/i18n/platform'
import { notifications, markRead, type NotificationList } from '@/api/platformEngage'

const pt = usePt()
const i18n = useI18nStore()
const router = useRouter()
const list = ref<NotificationList>({ unread: 0, items: [] })
const error = ref('')

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })

async function load() {
  try {
    list.value = await notifications()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function open(n: NotificationList['items'][number]) {
  try {
    if (!n.read) await markRead([n.id])
  } catch { /* opening the link matters more than the read flag */ }
  if (n.link && n.link.startsWith('/')) router.push(n.link)
  else await load()
}

async function readAll() {
  try {
    await markRead()
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
onMounted(load)
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <div class="flex items-center gap-3 my-3">
      <h1 class="text-display-sm font-bold tracking-tight flex-1">{{ pt('notifications') }}</h1>
      <button v-if="list.unread" class="btn-outlined" @click="readAll">{{ pt('markAllRead') }}</button>
    </div>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!list.items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noNotifications') }}</p>
    <ul class="space-y-2">
      <li v-for="n in list.items" :key="n.id">
        <button class="w-full text-start card-filled p-3" :class="{ 'ring-2': !n.read }" @click="open(n)">
          <div class="break-words" :class="{ 'font-bold': !n.read }">{{ formatNotification(pt, n.kind, n.data) }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ fmt(n.created_at) }}</div>
        </button>
      </li>
    </ul>
  </div>
</template>
