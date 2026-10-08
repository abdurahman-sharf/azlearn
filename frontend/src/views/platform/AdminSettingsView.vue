<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useBrandingStore } from '@/stores/branding'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import BackupPanel from '@/components/platform/BackupPanel.vue'
import {
  aiUsage, deleteLogo, getLegal, getSettings, saveBranding, saveLegal, saveSettings, testAi, uploadLogo,
  type Settings, type TestResult, type Usage,
} from '@/api/platformSettings'

const pt = usePt()
const brand = useBrandingStore()
const i18n = useI18nStore()

const settings = ref<Settings | null>(null)
const usage = ref<Usage | null>(null)
const loadError = ref('')

// One status line per section so a message appears next to the button that caused it.
type Section = 'ai' | 'caps' | 'brand' | 'teachers' | 'legalPrivacy' | 'legalTerms'
const msg = reactive<Record<Section, { ok: boolean; text: string } | null>>({ ai: null, caps: null, brand: null, teachers: null, legalPrivacy: null, legalTerms: null })
const busy = ref<Section | ''>('')

async function run(section: Section, fn: () => Promise<void>) {
  busy.value = section
  msg[section] = null
  try {
    await fn()
    msg[section] = { ok: true, text: pt('settingsSaved') }
  } catch (e) {
    msg[section] = { ok: false, text: platformErrorMessage(pt, e) }
  } finally {
    busy.value = ''
  }
}

// ── AI
const ai = reactive({ endpoint: '', model: '', apiKey: '' })
const testing = ref(false)
const testResult = ref<TestResult | null>(null)
const sourceKey = computed<PlatformKey>(() => ({ db: 'settingsSourceDb', env: 'settingsSourceEnv', none: 'settingsSourceNone' } as const)[settings.value?.ai.source ?? 'none'])

function adopt(s: Settings) {
  settings.value = s
  ai.endpoint = s.ai.endpoint
  ai.model = s.ai.model
  ai.apiKey = ''
  caps.platform = s.cap_platform
  caps.admin = s.cap_admin
  teachersExams.value = s.teachers_can_create_exams
}
const saveAi = () => run('ai', async () => {
  // The key is only sent when something was typed; an empty box never overwrites the saved key.
  adopt(await saveSettings({ endpoint: ai.endpoint, model: ai.model, ...(ai.apiKey.trim() ? { api_key: ai.apiKey.trim() } : {}) }))
})
const clearKey = () => run('ai', async () => { adopt(await saveSettings({ api_key: '' })) })
async function runTest() {
  testing.value = true
  testResult.value = null
  msg.ai = null
  try {
    testResult.value = await testAi({ endpoint: ai.endpoint || undefined, model: ai.model || undefined, api_key: ai.apiKey.trim() || undefined })
  } catch (e) {
    msg.ai = { ok: false, text: platformErrorMessage(pt, e) }
  } finally {
    testing.value = false
  }
}

// ── caps & usage
const caps = reactive({ platform: 0, admin: 0 })
const saveCaps = () => run('caps', async () => {
  adopt(await saveSettings({ cap_platform: Number(caps.platform), cap_admin: Number(caps.admin) }))
  usage.value = await aiUsage()
})
const dayLabel = (d: number) => new Date(d * 86_400_000).toISOString().slice(0, 10)
const resetsAt = computed(() => (usage.value ? new Date(usage.value.resets_at).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { timeZone: 'UTC', dateStyle: 'medium', timeStyle: 'short' }) + ' UTC' : ''))

// ── identity
const brandForm = reactive({ name: '', color: '' })
const logoInput = ref<HTMLInputElement | null>(null)
const saveBrand = () => run('brand', async () => {
  await saveBranding({ name: brandForm.name, color: brandForm.color })
  await brand.load()
})
const clearColor = () => run('brand', async () => { brandForm.color = ''; await saveBranding({ color: '' }); await brand.load() })
const onLogo = (ev: Event) => run('brand', async () => {
  const input = ev.target as HTMLInputElement
  const f = input.files?.[0]
  input.value = ''
  if (!f) return
  await uploadLogo(f)
  await brand.load()
})
const removeLogo = () => run('brand', async () => { await deleteLogo(); await brand.load() })

// ── teachers
const teachersExams = ref(false)
const saveTeachers = () => run('teachers', async () => { adopt(await saveSettings({ teachers_can_create_exams: teachersExams.value })) })

// ── legal
const legal = reactive({ privacy: '', terms: '' })
const saveLegalPage = (slug: 'privacy' | 'terms') => run(slug === 'privacy' ? 'legalPrivacy' : 'legalTerms', async () => {
  await saveLegal(slug, legal[slug])
  await brand.load()
})

onMounted(async () => {
  try {
    const [s, u] = await Promise.all([getSettings(), aiUsage(), brand.load()])
    adopt(s)
    usage.value = u
    brandForm.name = brand.config.name ?? ''
    brandForm.color = brand.config.color ?? ''
    legal.privacy = await getLegal('privacy')
    legal.terms = await getLegal('terms')
  } catch (e) {
    loadError.value = platformErrorMessage(pt, e)
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto pb-12 space-y-6" data-testid="settings-page">
    <h1 class="text-display-sm font-bold tracking-tight">{{ pt('settingsTitle') }}</h1>
    <p v-if="loadError" role="alert" style="color: rgb(var(--md-error))">{{ loadError }}</p>

    <template v-if="settings">
      <!-- AI -->
      <section class="card-filled p-5 space-y-4" data-testid="sec-ai">
        <div>
          <h2 class="text-title-md font-bold">{{ pt('settingsAiTitle') }}</h2>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsAiDesc') }}</p>
        </div>
        <form class="space-y-3" @submit.prevent="saveAi">
          <label class="block">
            <span class="text-label-lg">{{ pt('settingsEndpoint') }}</span>
            <input v-model="ai.endpoint" dir="ltr" type="url" class="input-outlined w-full mt-1" :placeholder="settings.ai.effective_endpoint || 'https://api.openai.com/v1'" data-testid="ai-endpoint" />
          </label>
          <label class="block">
            <span class="text-label-lg">{{ pt('settingsModel') }}</span>
            <input v-model="ai.model" dir="ltr" class="input-outlined w-full mt-1" :placeholder="settings.ai.effective_model || 'gpt-4o'" data-testid="ai-model" />
          </label>
          <label class="block">
            <span class="text-label-lg">{{ pt('settingsApiKey') }}</span>
            <span class="ms-2 text-body-sm px-2 py-0.5 rounded-full" data-testid="ai-key-state" style="background-color: rgb(var(--md-surface-container-high))">{{ settings.ai.key_saved ? pt('settingsKeySaved') : pt('settingsKeyNotSaved') }}</span>
            <input v-model="ai.apiKey" dir="ltr" type="password" autocomplete="off" class="input-outlined w-full mt-1" :placeholder="settings.ai.key_saved ? pt('settingsKeyReplace') : ''" data-testid="ai-key" />
          </label>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="ai-source">{{ pt(sourceKey) }}</p>
          <div class="flex flex-wrap gap-2 items-center">
            <button class="btn-filled" :disabled="busy === 'ai'" data-testid="ai-save">{{ pt('save') }}</button>
            <button type="button" class="btn-tonal" :disabled="testing" data-testid="ai-test" @click="runTest">{{ testing ? pt('settingsTesting') : pt('settingsTest') }}</button>
            <button v-if="settings.ai.key_saved" type="button" class="btn-text" data-testid="ai-key-clear" @click="clearKey">{{ pt('settingsKeyClear') }}</button>
          </div>
          <p v-if="testResult" role="status" class="text-body-sm" data-testid="ai-test-result" :style="{ color: testResult.ok ? 'rgb(var(--azl-success-text, var(--md-on-surface)))' : 'rgb(var(--md-error))' }">
            {{ testResult.ok ? pt('settingsTestOk') : testResult.code === 'ai_not_configured' ? pt('settingsTestNotConfigured') : pt('settingsTestFailed') }}
            <template v-if="testResult.detail"> — <span dir="ltr" class="inline-block">{{ testResult.detail }}</span></template>
          </p>
          <p v-if="msg.ai" :role="msg.ai.ok ? 'status' : 'alert'" class="text-body-sm" :style="msg.ai.ok ? {} : { color: 'rgb(var(--md-error))' }" data-testid="ai-msg">{{ msg.ai.text }}</p>
        </form>
      </section>

      <!-- Caps -->
      <section class="card-filled p-5 space-y-4" data-testid="sec-caps">
        <div>
          <h2 class="text-title-md font-bold">{{ pt('settingsCapsTitle') }}</h2>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsCapsDesc') }}</p>
        </div>
        <div v-if="usage" class="grid grid-cols-2 gap-2">
          <div class="card-elevated p-3"><div class="text-body-sm">{{ pt('settingsUsagePlatform') }}</div><div class="text-title-lg font-bold" dir="ltr" data-testid="usage-platform">{{ usage.today_platform }} / {{ usage.cap_platform }}</div></div>
          <div class="card-elevated p-3"><div class="text-body-sm">{{ pt('settingsUsageMine') }}</div><div class="text-title-lg font-bold" dir="ltr" data-testid="usage-mine">{{ usage.today_mine }} / {{ usage.cap_admin }}</div></div>
        </div>
        <p v-if="usage" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsResets') }}: <span dir="ltr" class="inline-block">{{ resetsAt }}</span></p>
        <form class="grid grid-cols-2 gap-3" @submit.prevent="saveCaps">
          <label class="block"><span class="text-label-lg">{{ pt('settingsCapPlatform') }}</span>
            <input v-model.number="caps.platform" type="number" min="0" max="100000" class="input-outlined w-full mt-1" dir="ltr" data-testid="cap-platform" /></label>
          <label class="block"><span class="text-label-lg">{{ pt('settingsCapAdmin') }}</span>
            <input v-model.number="caps.admin" type="number" min="0" max="100000" class="input-outlined w-full mt-1" dir="ltr" data-testid="cap-admin" /></label>
          <div class="col-span-2 flex items-center gap-3">
            <button class="btn-filled" :disabled="busy === 'caps'" data-testid="caps-save">{{ pt('save') }}</button>
            <p v-if="msg.caps" :role="msg.caps.ok ? 'status' : 'alert'" class="text-body-sm" :style="msg.caps.ok ? {} : { color: 'rgb(var(--md-error))' }" data-testid="caps-msg">{{ msg.caps.text }}</p>
          </div>
        </form>
        <div v-if="usage">
          <h3 class="text-label-lg mb-1">{{ pt('settingsUsageHistory') }}</h3>
          <p v-if="!usage.days.length" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsNoUsage') }}</p>
          <ul class="text-body-sm space-y-1" data-testid="usage-days">
            <li v-for="d in usage.days" :key="d.day" class="flex justify-between"><span dir="ltr">{{ dayLabel(d.day) }}</span><span dir="ltr">{{ d.calls }}</span></li>
          </ul>
          <template v-if="usage.admins_today.length">
            <h3 class="text-label-lg mt-3 mb-1">{{ pt('settingsUsageByAdmin') }}</h3>
            <ul class="text-body-sm space-y-1">
              <li v-for="a in usage.admins_today" :key="a.user_id" class="flex justify-between"><span>{{ a.name }}</span><span dir="ltr">{{ a.calls }}</span></li>
            </ul>
          </template>
        </div>
      </section>

      <!-- Identity -->
      <section class="card-filled p-5 space-y-4" data-testid="sec-brand">
        <h2 class="text-title-md font-bold">{{ pt('settingsBrandTitle') }}</h2>
        <form class="space-y-3" @submit.prevent="saveBrand">
          <label class="block"><span class="text-label-lg">{{ pt('settingsBrandName') }}</span>
            <input v-model="brandForm.name" maxlength="60" class="input-outlined w-full mt-1" data-testid="brand-name-input" />
            <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsBrandNameHint') }}</span></label>
          <label class="block"><span class="text-label-lg">{{ pt('settingsBrandColor') }}</span>
            <span class="flex items-center gap-2 mt-1">
              <input v-model="brandForm.color" type="color" class="h-10 w-14 rounded cursor-pointer" aria-hidden="true" tabindex="-1" />
              <input v-model="brandForm.color" dir="ltr" maxlength="7" placeholder="#2563EB" class="input-outlined flex-1" data-testid="brand-color-input" />
              <button type="button" class="btn-text" data-testid="brand-color-clear" @click="clearColor">{{ pt('settingsBrandColorClear') }}</button>
            </span>
            <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsBrandColorHint') }}</span></label>
          <div class="flex items-center gap-3">
            <button class="btn-filled" :disabled="busy === 'brand'" data-testid="brand-save">{{ pt('save') }}</button>
            <p v-if="msg.brand" :role="msg.brand.ok ? 'status' : 'alert'" class="text-body-sm" :style="msg.brand.ok ? {} : { color: 'rgb(var(--md-error))' }" data-testid="brand-msg">{{ msg.brand.text }}</p>
          </div>
        </form>
        <div class="space-y-2">
          <div class="text-label-lg">{{ pt('settingsLogo') }}</div>
          <img v-if="brand.logo" :src="brand.logo" alt="" class="h-16 w-16 rounded-xl object-contain" data-testid="logo-preview" />
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsLogoHint') }}</p>
          <div class="flex gap-2">
            <button type="button" class="btn-tonal" data-testid="logo-upload" @click="logoInput?.click()">{{ pt('settingsLogoUpload') }}</button>
            <button v-if="brand.logo" type="button" class="btn-text" data-testid="logo-remove" @click="removeLogo">{{ pt('settingsLogoRemove') }}</button>
            <input ref="logoInput" type="file" accept="image/png,image/jpeg" class="hidden" data-testid="logo-file" @change="onLogo" />
          </div>
        </div>
      </section>

      <!-- Teachers -->
      <section class="card-filled p-5 space-y-3" data-testid="sec-teachers">
        <h2 class="text-title-md font-bold">{{ pt('settingsTeachersTitle') }}</h2>
        <label class="flex items-start gap-3">
          <input v-model="teachersExams" type="checkbox" class="mt-1 h-5 w-5" data-testid="teachers-exams" />
          <span><span class="block">{{ pt('settingsTeachersExams') }}</span><span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsTeachersExamsHint') }}</span></span>
        </label>
        <div class="flex items-center gap-3">
          <button class="btn-filled" :disabled="busy === 'teachers'" data-testid="teachers-save" @click="saveTeachers">{{ pt('save') }}</button>
          <p v-if="msg.teachers" role="status" class="text-body-sm" data-testid="teachers-msg">{{ msg.teachers.text }}</p>
        </div>
      </section>

      <!-- Backup (full management lives on the system-status page) -->
      <section class="card-filled p-5 space-y-3" data-testid="sec-backup">
        <div>
          <h2 class="text-title-md font-bold">{{ pt('bkTitle') }}</h2>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('bkDesc') }}</p>
        </div>
        <BackupPanel compact />
        <router-link to="/platform/admin/system" class="inline-block text-body-md underline" data-testid="backup-manage">{{ pt('bkManage') }}</router-link>
      </section>

      <!-- Legal -->
      <section class="card-filled p-5 space-y-4" data-testid="sec-legal">
        <div>
          <h2 class="text-title-md font-bold">{{ pt('settingsLegalTitle') }}</h2>
          <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('settingsLegalDesc') }}</p>
        </div>
        <div v-for="slug in (['privacy', 'terms'] as const)" :key="slug" class="space-y-2">
          <label class="block"><span class="text-label-lg">{{ pt(slug === 'privacy' ? 'settingsLegalPrivacy' : 'settingsLegalTerms') }}</span>
            <textarea v-model="legal[slug]" rows="8" maxlength="20000" dir="auto" class="input-outlined w-full mt-1" :data-testid="'legal-' + slug"></textarea></label>
          <div class="flex items-center gap-3">
            <button class="btn-filled" :disabled="busy === (slug === 'privacy' ? 'legalPrivacy' : 'legalTerms')" :data-testid="'legal-save-' + slug" @click="saveLegalPage(slug)">{{ pt('save') }}</button>
            <p v-if="msg[slug === 'privacy' ? 'legalPrivacy' : 'legalTerms']" role="status" class="text-body-sm" :data-testid="'legal-msg-' + slug">{{ msg[slug === 'privacy' ? 'legalPrivacy' : 'legalTerms']!.text }}</p>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>
