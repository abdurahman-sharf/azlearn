<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { useAdminStats } from '@/lib/adminStats'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { fmtBytes, fmtDuration, systemInfo, type SystemInfo, type SystemWarning } from '@/api/platformSystem'
import BackupPanel from '@/components/platform/BackupPanel.vue'

const pt = usePt()
const i18n = useI18nStore()
const { refresh: refreshBadges } = useAdminStats()
const info = ref<SystemInfo | null>(null)
const error = ref('')
const loading = ref(false)

const fmt = (ms: number) => new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' })
const warnKey: Record<SystemWarning, PlatformKey> = { no_backup: 'sysWarnNoBackup', backup_overdue: 'sysWarnOverdue', last_backup_failed: 'sysWarnFailed', low_disk: 'sysWarnLowDisk' }
const fill = (s: string, v: Record<string, string | number>) => Object.entries(v).reduce((t, [k, x]) => t.replace(`{${k}}`, String(x)), s)

async function load() {
  loading.value = true
  error.value = ''
  try {
    info.value = await systemInfo()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}
async function changed() {
  await load()
  void refreshBadges() // the sidebar badge counts the same warnings
}

const usedPct = computed(() => {
  const i = info.value
  return i?.disk_total_bytes && i.disk_free_bytes !== null ? Math.round((1 - i.disk_free_bytes / i.disk_total_bytes) * 100) : null
})
const schedule = computed(() => {
  const i = info.value
  if (!i) return ''
  return i.nightly_hour === null
    ? fill(pt('sysScheduleOff'), { manual: i.keep_manual })
    : fill(pt('sysScheduleOn'), { hour: String(i.nightly_hour).padStart(2, '0'), keep: i.keep_nightly, manual: i.keep_manual })
})

const cards = computed(() => {
  const i = info.value
  if (!i) return []
  return [
    { id: 'version', label: 'sysVersion' as PlatformKey, value: i.version },
    { id: 'uptime', label: 'sysUptime' as PlatformKey, value: fmtDuration(i.uptime_sec) },
    { id: 'db', label: 'sysDb' as PlatformKey, value: fmtBytes(i.db_bytes) },
    { id: 'files', label: 'sysFiles' as PlatformKey, value: `${fmtBytes(i.files_bytes)} · ${i.files_count} ${pt('sysFileUnit')}` },
    { id: 'backups', label: 'sysBackups' as PlatformKey, value: `${i.backups_count} · ${fmtBytes(i.backups_bytes)}` },
    { id: 'sessions', label: 'sysSessions' as PlatformKey, value: String(i.sessions_active) },
    { id: 'users', label: 'sysUsers' as PlatformKey, value: String(i.users) },
  ]
})
onMounted(load)
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-6" data-testid="system-page">
    <div class="flex items-center gap-3">
      <h1 class="text-display-sm font-bold tracking-tight flex-1">{{ pt('sysTitle') }}</h1>
      <button class="btn-outlined" :disabled="loading" data-testid="sys-refresh" @click="load">{{ pt('sysRefresh') }}</button>
    </div>
    <p v-if="error" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <template v-if="info">
      <section v-if="info.warnings.length" class="rounded-2xl p-4 space-y-1" role="alert" style="background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))" data-testid="sys-warnings">
        <h2 class="text-title-sm font-bold">{{ pt('sysWarnings') }}</h2>
        <ul class="list-disc ps-5">
          <li v-for="w in info.warnings" :key="w" :data-testid="'sys-warning-' + w">{{ pt(warnKey[w]) }}</li>
        </ul>
      </section>

      <section class="grid grid-cols-2 md:grid-cols-4 gap-3" data-testid="sys-cards">
        <div v-for="c in cards" :key="c.id" class="card-filled p-4" :data-testid="'sys-card-' + c.id">
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(c.label) }}</div>
          <div class="text-title-md font-bold break-words" dir="ltr" :style="{ textAlign: i18n.locale === 'ar' ? 'right' : 'left' }" :data-testid="'sys-value-' + c.id">{{ c.value }}</div>
        </div>
        <div class="card-filled p-4 col-span-2" data-testid="sys-card-disk">
          <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('sysDisk') }}</div>
          <template v-if="info.disk_free_bytes !== null && info.disk_total_bytes">
            <div class="text-title-md font-bold" dir="ltr" :style="{ textAlign: i18n.locale === 'ar' ? 'right' : 'left' }" data-testid="sys-value-disk">{{ fmtBytes(info.disk_free_bytes) }} / {{ fmtBytes(info.disk_total_bytes) }}</div>
            <div class="mt-2 h-2 rounded-full overflow-hidden" role="progressbar" :aria-valuenow="usedPct ?? 0" aria-valuemin="0" aria-valuemax="100" :aria-label="pt('sysDisk')" style="background-color: rgb(var(--md-surface-container-highest))">
              <div class="h-full" :style="{ width: (usedPct ?? 0) + '%', backgroundColor: info.warnings.includes('low_disk') ? 'rgb(var(--md-error))' : 'rgb(var(--md-primary))' }"></div>
            </div>
          </template>
          <div v-else class="text-title-md font-bold" data-testid="sys-value-disk">{{ pt('sysDiskUnknown') }}</div>
        </div>
      </section>

      <section class="card-filled p-5 space-y-4" data-testid="sys-backup">
        <div>
          <h2 class="text-title-md font-bold">{{ pt('bkTitle') }}</h2>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('bkDesc') }}</p>
        </div>
        <dl class="grid gap-x-6 gap-y-2 md:grid-cols-[auto_1fr] text-body-md">
          <dt class="font-semibold">{{ pt('sysSchedule') }}</dt>
          <dd data-testid="sys-schedule">{{ schedule }}</dd>
          <dt class="font-semibold">{{ pt('sysLastBackup') }}</dt>
          <dd data-testid="sys-last-backup">
            <template v-if="info.last_backup">{{ fmt(info.last_backup.created_at) }} · <span dir="ltr" class="inline-block">{{ fmtBytes(info.last_backup.size) }}</span></template>
            <template v-else>{{ pt('sysNoBackup') }}</template>
          </dd>
          <template v-if="info.last_failure">
            <dt class="font-semibold">{{ pt('sysLastFailure') }}</dt>
            <dd data-testid="sys-last-failure">{{ fmt(info.last_failure.created_at) }} — <span dir="ltr" class="inline-block break-all">{{ info.last_failure.error }}</span></dd>
          </template>
          <dt class="font-semibold">{{ pt('sysBackupDir') }}</dt>
          <dd><span dir="ltr" class="inline-block break-all" data-testid="sys-backup-dir">{{ info.backup_dir }}</span></dd>
        </dl>
        <BackupPanel @changed="changed" />
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="sys-offsite">{{ pt('bkOffsite') }}</p>
        <details class="rounded-xl p-3" style="background-color: rgb(var(--md-surface-container-high))" data-testid="sys-restore">
          <summary class="font-semibold cursor-pointer">{{ pt('bkRestoreTitle') }}</summary>
          <p class="text-body-sm mt-2">{{ pt('bkRestoreIntro') }}</p>
          <div class="text-label-lg mt-3">{{ pt('bkRestoreDocker') }}</div>
          <pre dir="ltr" class="text-body-sm overflow-x-auto rounded-lg p-3 mt-1" style="background-color: rgb(var(--md-surface-container-highest)); text-align: left">docker compose stop exameow
docker compose run --rm exameow ./server restore /app/data/backups/&lt;file&gt;.zip         # verify only
docker compose run --rm exameow ./server restore /app/data/backups/&lt;file&gt;.zip --yes   # replace the data
docker compose start exameow</pre>
          <div class="text-label-lg mt-3">{{ pt('bkRestoreBare') }}</div>
          <pre dir="ltr" class="text-body-sm overflow-x-auto rounded-lg p-3 mt-1" style="background-color: rgb(var(--md-surface-container-highest)); text-align: left"># stop the server, then (same environment variables as the server)
./exameow-server restore backups/&lt;file&gt;.zip          # verify only
./exameow-server restore backups/&lt;file&gt;.zip --yes    # replace the data</pre>
        </details>
      </section>
    </template>
  </div>
</template>
