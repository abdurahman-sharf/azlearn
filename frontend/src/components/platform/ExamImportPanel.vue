<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Question } from '@exameow/shared'
import { ArrowDownTrayIcon, ExclamationTriangleIcon } from '@heroicons/vue/24/outline'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { downloadTemplate, ImportFileError, parseQuestionFile, planImport, type ParsedFile, type RowIssue } from '@/utils/examImport'

// Adds the questions of an Excel/CSV file to the exam being built: the file is read in the browser, every row is
// checked with the rules of the editor and the server, problems are listed by their real sheet row, and only the
// good rows are added.
const props = defineProps<{ existing: Question[]; room: number }>()
const emit = defineEmits<{ imported: [Question[]] }>()
const pt = usePt()

const parsed = ref<ParsedFile | null>(null)
const fileName = ref('')
const skipDup = ref(true)
const error = ref('')
const notice = ref('')
const busy = ref(false)
const input = ref<HTMLInputElement | null>(null)
const MAX_LISTED = 40

const plan = computed(() => (parsed.value ? planImport(parsed.value, props.existing, props.room, skipDup.value) : null))
const canAdd = computed(() => !!plan.value && plan.value.good.length > 0 && plan.value.overflow === 0)

async function onFile(ev: Event) {
  const el = ev.target as HTMLInputElement
  const file = el.files?.[0]
  parsed.value = null
  error.value = ''
  notice.value = ''
  if (!file) return
  fileName.value = file.name
  busy.value = true
  try {
    parsed.value = await parseQuestionFile(file.name, await file.arrayBuffer())
  } catch (e) {
    error.value = pt(`xiErr_${e instanceof ImportFileError ? e.code : 'unreadable'}` as PlatformKey)
  } finally {
    busy.value = false
  }
}

function add() {
  if (!plan.value || !canAdd.value) return
  const qs = plan.value.good.map((g) => g.question)
  emit('imported', qs)
  notice.value = `${pt('xiAdded')} ${qs.length}`
  parsed.value = null
  if (input.value) input.value.value = ''
}

const reason = (issue: RowIssue) =>
  pt((issue === 'empty_stem' ? 'xiIssue_empty_stem' : issue === 'duplicate_file' ? 'xiIssue_duplicate_file' : issue === 'duplicate_exam' ? 'xiIssue_duplicate_exam' : `exProblem_${issue}`) as PlatformKey)
</script>

<template>
  <section class="card-filled p-5 space-y-3" data-testid="exam-import">
    <div class="flex flex-wrap items-start justify-between gap-2">
      <div class="min-w-0">
        <h2 class="text-title-md font-bold">{{ pt('xiTitle') }}</h2>
        <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('xiHint') }}</p>
      </div>
      <button type="button" class="btn-text shrink-0" data-testid="exim-template" @click="downloadTemplate">
        <ArrowDownTrayIcon class="w-5 h-5" aria-hidden="true" /> {{ pt('xiTemplate') }}
      </button>
    </div>

    <label class="block">
      <span class="sr-only">{{ pt('xiChoose') }}</span>
      <input ref="input" type="file" accept=".xlsx,.xls,.csv" class="input-outlined w-full" data-testid="exim-file" @change="onFile" />
    </label>

    <p v-if="busy" role="status" class="text-body-sm">{{ pt('xiReading') }}</p>
    <p v-if="error" role="alert" class="text-body-md font-semibold" style="color: rgb(var(--md-error))" data-testid="exim-error">{{ error }}</p>
    <p v-if="notice" role="status" class="text-body-md font-semibold" data-testid="exim-notice">{{ notice }}</p>

    <template v-if="plan">
      <div class="flex flex-wrap gap-x-5 gap-y-1 text-body-md" data-testid="exim-summary">
        <span dir="auto" class="font-semibold">{{ fileName }}</span>
        <span>{{ pt('xiSummaryOk') }}: <b dir="ltr" class="inline-block" data-testid="exim-good">{{ plan.good.length }}</b></span>
        <span>{{ pt('xiSummaryBad') }}: <b dir="ltr" class="inline-block" data-testid="exim-bad">{{ plan.problems.length }}</b></span>
      </div>
      <label class="flex items-center gap-2"><input v-model="skipDup" type="checkbox" class="h-5 w-5" data-testid="exim-skipdup" /> {{ pt('xiSkipDup') }}</label>

      <p v-if="plan.overflow > 0" role="alert" class="text-body-md font-semibold" style="color: rgb(var(--md-error))" data-testid="exim-overflow">
        {{ pt('xiOverflow') }} <b dir="ltr" class="inline-block">{{ Math.max(0, props.room) }}</b>
      </p>

      <ul v-if="plan.problems.length" class="space-y-1 rounded-xl p-3" style="background-color: rgb(var(--md-surface))" :aria-label="pt('xiProblems')" data-testid="exim-problems">
        <li v-for="p in plan.problems.slice(0, MAX_LISTED)" :key="p.rowNumber + p.issue" class="flex items-start gap-2 text-body-sm">
          <ExclamationTriangleIcon class="w-4 h-4 mt-0.5 shrink-0" aria-hidden="true" />
          <span>
            <b dir="ltr" class="inline-block">{{ pt('xiRow') }} {{ p.rowNumber }}</b> — {{ reason(p.issue) }}
            <span v-if="p.stem" dir="auto" class="block" style="color: rgb(var(--md-on-surface-variant))">{{ p.stem }}</span>
          </span>
        </li>
        <li v-if="plan.problems.length > MAX_LISTED" class="text-body-sm font-semibold">{{ pt('xiMore') }} <span dir="ltr" class="inline-block">{{ plan.problems.length - MAX_LISTED }}</span></li>
      </ul>

      <button type="button" class="btn-filled" :disabled="!canAdd" data-testid="exim-add" @click="add">
        {{ pt('xiAdd') }} (<span dir="ltr" class="inline-block">{{ plan.good.length }}</span>)
      </button>
    </template>
  </section>
</template>
