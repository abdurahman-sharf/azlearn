<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { importBank, type ImportResult } from '@/api/platformBank'
import { parseCSV, parseExcel } from '@/utils/importParser'

const props = defineProps<{ subjectId: string }>()
const emit = defineEmits<{ done: []; close: [] }>()
const pt = usePt()

interface Candidate { id: string; name: string; questions: Record<string, unknown>[] }
const tab = ref<'local' | 'file'>('local')
const locals = ref<Candidate[]>([])
const chosen = ref('')
const fileQuestions = ref<Record<string, unknown>[] | null>(null)
const fileName = ref('')
const skipDup = ref(true)
const busy = ref(false)
const error = ref('')
const result = ref<ImportResult | null>(null)

onMounted(() => {
  // Question banks made by the generator / practice mode live in this browser's localStorage.
  try {
    const raw = JSON.parse(localStorage.getItem('exameow-banks') ?? '[]')
    if (Array.isArray(raw)) {
      locals.value = raw
        .filter((b) => b && Array.isArray(b.questions) && b.questions.length)
        .map((b) => ({ id: String(b.id), name: String(b.name ?? ''), questions: b.questions }))
    }
  } catch { /* unreadable storage: nothing to offer */ }
})

async function onFile(ev: Event) {
  const input = ev.target as HTMLInputElement
  const f = input.files?.[0]
  input.value = ''
  fileQuestions.value = null
  error.value = ''
  if (!f) return
  fileName.value = f.name
  try {
    const parsed = /\.(xlsx|xls)$/i.test(f.name) ? parseExcel(await f.arrayBuffer(), f.name) : parseCSV(await f.text())
    if (!parsed.questions.length) throw new Error('empty')
    fileQuestions.value = parsed.questions as unknown as Record<string, unknown>[]
  } catch {
    error.value = pt('bankImportFileError')
  }
}

const picked = (): Record<string, unknown>[] => (tab.value === 'local' ? locals.value.find((b) => b.id === chosen.value)?.questions : fileQuestions.value) ?? []

async function run() {
  const qs = picked()
  if (!qs.length || !props.subjectId) return
  busy.value = true
  error.value = ''
  result.value = null
  try {
    // Only the fields the bank stores are sent (browser questions carry extra ones).
    const items = qs.map((q) => ({ type: q.type, stem: q.stem, options: q.options ?? [], answer: q.answer ?? '', analysis: q.analysis ?? '', chapter: typeof q.chapter === 'string' ? q.chapter : undefined, difficulty: q.difficulty }))
    result.value = await importBank(props.subjectId, items, skipDup.value)
    emit('done')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}
const reason = (code: string) => platformErrorMessage(pt, new PlatformError(code, 400))
</script>

<template>
  <section class="card-filled p-5 space-y-4" data-testid="import-panel">
    <div class="flex items-center justify-between">
      <h2 class="text-title-md font-bold">{{ pt('bankImportTitle') }}</h2>
      <button class="btn-text" data-testid="import-close" @click="emit('close')">{{ pt('bankClose') }}</button>
    </div>
    <div class="flex gap-2">
      <button :class="tab === 'local' ? 'btn-filled' : 'btn-outlined'" data-testid="tab-local" @click="tab = 'local'">{{ pt('bankImportLocal') }}</button>
      <button :class="tab === 'file' ? 'btn-filled' : 'btn-outlined'" data-testid="tab-file" @click="tab = 'file'">{{ pt('bankImportFile') }}</button>
    </div>

    <div v-if="tab === 'local'" class="space-y-2">
      <p v-if="!locals.length" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="no-locals">{{ pt('bankImportNoLocal') }}</p>
      <label v-for="b in locals" :key="b.id" class="card-elevated flex items-center gap-3 p-3 cursor-pointer">
        <input v-model="chosen" type="radio" :value="b.id" :data-testid="'local-' + b.id" />
        <span class="flex-1 min-w-0 truncate">{{ b.name }}</span>
        <span class="text-body-sm" dir="ltr">{{ b.questions.length }}</span>
        <span class="text-body-sm">{{ pt('bankImportQuestions') }}</span>
      </label>
    </div>
    <div v-else class="space-y-2">
      <input type="file" accept=".csv,.xlsx,.xls" data-testid="import-file" @change="onFile" />
      <p v-if="fileQuestions" class="text-body-sm" data-testid="file-count"><span dir="ltr" class="inline-block">{{ fileName }}</span> — <span dir="ltr" class="inline-block">{{ fileQuestions.length }}</span> {{ pt('bankImportQuestions') }}</p>
    </div>

    <label class="flex items-center gap-2"><input v-model="skipDup" type="checkbox" data-testid="import-skip" /> {{ pt('bankImportSkipDup') }}</label>
    <button class="btn-filled" :disabled="busy || !subjectId || !picked().length" data-testid="import-run" @click="run">{{ pt('bankImportRun') }}</button>
    <p v-if="error" role="alert" class="text-body-sm" style="color: rgb(var(--md-error))" data-testid="import-error">{{ error }}</p>

    <div v-if="result" role="status" class="space-y-2" data-testid="import-result">
      <ul class="text-body-md">
        <li>{{ pt('bankImportCreated') }}: <b dir="ltr" class="inline-block" data-testid="res-created">{{ result.created }}</b></li>
        <li v-if="result.skipped_duplicates">{{ pt('bankImportSkipped') }}: <b dir="ltr" class="inline-block" data-testid="res-skipped">{{ result.skipped_duplicates }}</b></li>
        <li v-if="result.duplicates_kept">{{ pt('bankImportKept') }}: <b dir="ltr" class="inline-block">{{ result.duplicates_kept }}</b></li>
        <li v-if="result.rejected.length">{{ pt('bankImportRejected') }}: <b dir="ltr" class="inline-block" data-testid="res-rejected">{{ result.rejected.length }}</b></li>
      </ul>
      <details v-if="result.rejected.length">
        <summary class="cursor-pointer text-body-sm">{{ pt('bankImportRejectedTitle') }}</summary>
        <ul class="text-body-sm mt-1 space-y-1">
          <li v-for="r in result.rejected.slice(0, 50)" :key="r.index"><span dir="ltr" class="inline-block">#{{ r.index + 1 }}</span> — {{ reason(r.error) }}</li>
        </ul>
      </details>
    </div>
  </section>
</template>
