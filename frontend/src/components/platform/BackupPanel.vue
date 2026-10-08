<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { createBackup, downloadBackup, fmtBytes, listBackups, type BackupInfo } from '@/api/platformSystem'

const props = defineProps<{ compact?: boolean }>()
const emit = defineEmits<{ (e: 'changed'): void }>()

const pt = usePt()
const i18n = useI18nStore()
const items = ref<BackupInfo[]>([])
const loaded = ref(false)
const busy = ref('')
const msg = ref<{ ok: boolean; text: string } | null>(null)

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const shown = () => (props.compact ? items.value.slice(0, 1) : items.value)

async function load() {
  try {
    items.value = await listBackups()
  } catch (e) {
    msg.value = { ok: false, text: platformErrorMessage(pt, e) }
  } finally {
    loaded.value = true
  }
}

async function make(andDownload: boolean) {
  busy.value = andDownload ? 'download' : 'create'
  msg.value = null
  try {
    const b = await createBackup()
    if (andDownload) await downloadBackup(b)
    msg.value = { ok: true, text: pt('bkCreated') }
  } catch (e) {
    msg.value = { ok: false, text: platformErrorMessage(pt, e) }
  } finally {
    busy.value = ''
    await load() // a failed attempt is recorded by the server too
    emit('changed')
  }
}

async function download(b: BackupInfo) {
  busy.value = b.id
  msg.value = null
  try {
    await downloadBackup(b)
  } catch (e) {
    msg.value = { ok: false, text: platformErrorMessage(pt, e) }
  } finally {
    busy.value = ''
  }
}

onMounted(load)
defineExpose({ load })
</script>

<template>
  <div class="space-y-3" data-testid="backup-panel">
    <div class="flex flex-wrap gap-2 items-center">
      <button class="btn-filled" :disabled="!!busy" data-testid="backup-download-now" @click="make(true)">{{ busy === 'download' ? pt('bkWorking') : pt('bkDownloadNow') }}</button>
      <button v-if="!compact" class="btn-tonal" :disabled="!!busy" data-testid="backup-create" @click="make(false)">{{ busy === 'create' ? pt('bkWorking') : pt('bkCreateOnly') }}</button>
    </div>
    <p v-if="msg" :role="msg.ok ? 'status' : 'alert'" class="text-body-sm" :style="msg.ok ? {} : { color: 'rgb(var(--md-error))' }" data-testid="backup-msg">{{ msg.text }}</p>

    <p v-if="loaded && !items.length" class="text-body-md" style="color: rgb(var(--md-on-surface-variant))" data-testid="backup-empty">{{ pt('bkNone') }}</p>
    <ul v-else class="space-y-2" data-testid="backup-list">
      <li
        v-for="b in shown()"
        :key="b.id"
        class="rounded-xl p-3 flex flex-wrap items-center gap-x-4 gap-y-1"
        style="background-color: rgb(var(--md-surface-container-high))"
        data-testid="backup-row"
        :data-id="b.id"
        :data-ok="b.ok"
      >
        <span class="font-semibold">{{ fmt(b.created_at) }}</span>
        <span class="text-body-sm px-2 py-0.5 rounded-full" style="background-color: rgb(var(--md-surface-container-highest))">{{ b.kind === 'nightly' ? pt('bkKindNightly') : pt('bkKindManual') }}</span>
        <template v-if="b.ok">
          <span class="text-body-sm" dir="ltr">{{ fmtBytes(b.size) }}</span>
          <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ b.files_count }}</span> {{ pt('sysFileUnit') }}</span>
          <span v-if="!b.present" class="text-body-sm" style="color: rgb(var(--md-error))" data-testid="backup-missing">{{ pt('bkMissing') }}</span>
        </template>
        <span v-else class="text-body-sm break-words min-w-0" style="color: rgb(var(--md-error))" data-testid="backup-error">{{ pt('bkFailed') }} — <span dir="ltr" class="inline-block">{{ b.error }}</span></span>
        <button v-if="b.ok && b.present" class="btn-text ms-auto" :disabled="!!busy" data-testid="backup-row-download" @click="download(b)">{{ busy === b.id ? pt('bkWorking') : pt('bkDownload') }}</button>
      </li>
    </ul>
  </div>
</template>
