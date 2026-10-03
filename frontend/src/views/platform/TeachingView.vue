<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myTeaching, dropTeaching, type Teaching } from '@/api/platformLearning'

const pt = usePt()
const items = ref<Teaching[]>([])
const error = ref('')
const statusKey: Record<string, PlatformKey> = { pending: 'statusPending', approved: 'statusApproved', rejected: 'statusRejected' }

async function load() {
  try {
    items.value = await myTeaching()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
async function drop(t: Teaching) {
  if (!window.confirm(pt('confirmDelete'))) return
  try {
    await dropTeaching(t.subject_id)
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
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myTeaching') }}</h1>
    <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ pt('addTeachingHint') }}</p>
    <p v-if="error" class="text-body-sm mb-3" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-if="!items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noResults') }}</p>
    <ul class="space-y-3">
      <li v-for="t in items" :key="t.subject_id" class="card-filled p-4 flex items-center gap-3">
        <router-link :to="`/platform/subjects/${t.subject_id}`" class="flex-1 min-w-0">
          <div class="font-bold">{{ t.subject_name }}</div>
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ t.institution_name }}</div>
        </router-link>
        <span class="px-3 py-1 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-surface-container-high))">{{ pt(statusKey[t.status]!) }}</span>
        <button class="btn-text" @click="drop(t)">{{ pt('del') }}</button>
      </li>
    </ul>
  </div>
</template>
