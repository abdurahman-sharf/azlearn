<script setup lang="ts">
import { reactive, ref } from 'vue'
import type { Question } from '@exameow/shared'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { BANK_TYPES } from '@/api/platformBank'
import { generateQuestions } from '@/api/platformExamAdmin'
import { importBank } from '@/api/platformBank'

const props = defineProps<{ subjectId: string }>()
const emit = defineEmits<{ generated: [Question[]] }>()
const pt = usePt()

const file = ref<File | null>(null)
const counts = reactive<Record<string, number>>({ single_choice: 5, multi_choice: 0, true_false: 0, fill_blank: 0, short_answer: 0 })
const difficulty = ref<'easy' | 'medium' | 'hard'>('medium')
const language = ref('Arabic')
const autoChapter = ref(false)
const saveToBank = ref(false)
const busy = ref(false)
const error = ref('')
const done = ref('')

const total = () => Object.values(counts).reduce((a, b) => a + (Number(b) || 0), 0)
const onFile = (ev: Event) => { file.value = (ev.target as HTMLInputElement).files?.[0] ?? null }

async function run() {
  error.value = ''
  done.value = ''
  if (!file.value) { error.value = pt('exAiNeedFile'); return }
  if (total() < 1 || total() > 30 || Object.values(counts).some((n) => n < 0 || !Number.isInteger(Number(n)))) { error.value = pt('exAiNeedCount'); return }
  busy.value = true
  try {
    const qs = await generateQuestions(file.value, { type_counts: { ...counts }, difficulty: difficulty.value, language: language.value, auto_chapter: autoChapter.value })
    emit('generated', qs)
    done.value = `${pt('exAiDone')} ${qs.length}`
    if (saveToBank.value && props.subjectId) {
      // optional: keep the generated questions for reuse (duplicates are skipped, bad ones reported by the bank)
      await importBank(props.subjectId, qs.map((q) => ({ type: q.type, stem: q.stem, options: q.options, answer: q.answer, analysis: q.analysis, chapter: q.chapter, difficulty: q.difficulty })), true)
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="card-filled p-5 space-y-4" data-testid="ai-panel">
    <h3 class="text-title-md font-bold">{{ pt('exSourceAi') }}</h3>
    <label class="block">
      <span class="text-label-lg">{{ pt('exAiFile') }}</span>
      <input type="file" accept=".pdf,.docx,.xlsx,.xls,.pptx,.epub,.odt,.txt,.md,.csv,.html,.htm" class="block mt-1" data-testid="ai-file" @change="onFile" />
      <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('exAiFileHint') }}</span>
    </label>
    <fieldset>
      <legend class="text-label-lg">{{ pt('exAiCounts') }}</legend>
      <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 mt-1">
        <label v-for="t in BANK_TYPES" :key="t" class="flex items-center justify-between gap-2">
          <span class="text-body-md">{{ pt(`bank_t_${t}` as PlatformKey) }}</span>
          <input v-model.number="counts[t]" type="number" min="0" max="30" class="input-outlined w-20" dir="ltr" :data-testid="'ai-count-' + t" />
        </label>
      </div>
    </fieldset>
    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
      <label class="block"><span class="text-label-lg">{{ pt('exAiDifficulty') }}</span>
        <select v-model="difficulty" class="input-outlined w-full mt-1" data-testid="ai-difficulty">
          <option value="easy">{{ pt('bank_d_easy') }}</option><option value="medium">{{ pt('bank_d_medium') }}</option><option value="hard">{{ pt('bank_d_hard') }}</option>
        </select></label>
      <label class="block"><span class="text-label-lg">{{ pt('exAiLanguage') }}</span>
        <select v-model="language" class="input-outlined w-full mt-1" data-testid="ai-language">
          <option value="Arabic">{{ pt('exAiLangAr') }}</option><option value="English">{{ pt('exAiLangEn') }}</option>
        </select></label>
    </div>
    <label class="flex items-center gap-2"><input v-model="autoChapter" type="checkbox" data-testid="ai-chapter" /> {{ pt('exAiAutoChapter') }}</label>
    <label class="flex items-center gap-2"><input v-model="saveToBank" type="checkbox" data-testid="ai-save-bank" /> {{ pt('exAiSaveBank') }}</label>
    <p class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('exAiUsage') }}</p>
    <button class="btn-filled" :disabled="busy" data-testid="ai-run" @click="run">{{ busy ? pt('exAiGenerating') : pt('exAiGenerate') }}</button>
    <p v-if="done" role="status" class="text-body-md" data-testid="ai-done">{{ done }}</p>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="ai-error">{{ error }}</p>
  </section>
</template>
