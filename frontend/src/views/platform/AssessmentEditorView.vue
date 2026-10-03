<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePracticeStore } from '@/stores/practice'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type Teaching } from '@/api/platformLearning'
import { createAssessment, updateAssessment, deleteAssessment, getAssessment } from '@/api/platformExams'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const practice = usePracticeStore()
const id = route.params.id as string | undefined
const isEdit = computed(() => !!id)

const subjects = ref<Teaching[]>([])
const subjectId = ref('')
const bankId = ref('')
const title = ref('')
const description = ref('')
const duration = ref<number | null>(null)
const opensAt = ref('')
const closesAt = ref('')
const maxAttempts = ref(1)
const showAnswers = ref(true)
const status = ref<'draft' | 'published'>('draft')
const error = ref('')
const msg = ref('')
const locked = ref(false)

const bank = computed(() => practice.banks.find(b => b.id === bankId.value))
const toMs = (v: string) => (v ? new Date(v).getTime() : undefined)
const toLocal = (ms: number | null) => (ms ? new Date(ms - new Date(ms).getTimezoneOffset() * 60000).toISOString().slice(0, 16) : '')

onMounted(async () => {
  try {
    subjects.value = (await myTeaching()).filter(t => t.status === 'approved')
    if (id) {
      const a = await getAssessment(id)
      subjectId.value = a.subject_id; title.value = a.title; description.value = a.description ?? ''
      duration.value = a.duration_min; opensAt.value = toLocal(a.opens_at); closesAt.value = toLocal(a.closes_at)
      maxAttempts.value = a.max_attempts; showAnswers.value = a.show_answers; status.value = a.status
      locked.value = a.attempt_count > 0
    } else {
      subjectId.value = (route.query.subject as string) || subjects.value[0]?.subject_id || ''
      bankId.value = practice.banks[0]?.id ?? ''
      title.value = practice.banks[0]?.name ?? ''
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

function onBank() {
  if (!title.value || practice.banks.some(b => b.name === title.value)) title.value = bank.value?.name ?? ''
}

async function save() {
  error.value = msg.value = ''
  const common = {
    title: title.value, description: description.value || undefined,
    duration_min: duration.value || undefined, opens_at: toMs(opensAt.value), closes_at: toMs(closesAt.value),
    max_attempts: maxAttempts.value, show_answers: showAnswers.value, status: status.value,
  }
  try {
    if (id) {
      await updateAssessment(id, {
        ...common,
        clear_duration: !duration.value || undefined,
        clear_window: (!opensAt.value && !closesAt.value) || undefined,
      })
      msg.value = pt('saved')
    } else {
      if (!bank.value) return
      const a = await createAssessment({ ...common, subject_id: subjectId.value, questions: bank.value.questions })
      router.replace(`/platform/assessments/${a.id}`)
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove() {
  if (!id || !window.confirm(pt('confirmDelete'))) return
  try {
    await deleteAssessment(id)
    router.replace('/platform/my-content')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newAssessment') }}</h1>
    <p v-if="!subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noApprovedSubjects') }}</p>
    <p v-else-if="!isEdit && !practice.banks.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('noBanks') }} <router-link to="/generate" class="underline">/generate</router-link>
    </p>

    <form v-else class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <template v-if="!isEdit">
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
          <select v-model="subjectId" required class="input-outlined mt-1 w-full">
            <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
          </select>
        </label>
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('chooseBank') }}</span>
          <select v-model="bankId" required class="input-outlined mt-1 w-full" data-testid="bank" @change="onBank">
            <option v-for="b in practice.banks" :key="b.id" :value="b.id">{{ b.name }} ({{ b.questions.length }} {{ pt('questionsCount') }})</option>
          </select>
        </label>
      </template>
      <p v-else-if="locked" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('hasAttempts') }}</p>

      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="2" class="input-outlined mt-1 w-full"></textarea>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('durationOptional') }}</span>
        <input v-model.number="duration" type="number" min="1" max="480" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('maxAttempts') }}</span>
        <input v-model.number="maxAttempts" required type="number" min="1" max="10" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('opensAt') }}</span>
        <input v-model="opensAt" type="datetime-local" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('closesAt') }}</span>
        <input v-model="closesAt" type="datetime-local" class="input-outlined mt-1 w-full" />
      </label>
      <label class="flex items-center gap-2">
        <input v-model="showAnswers" type="checkbox" />
        <span>{{ pt('showAnswers') }}</span>
      </label>
      <select v-model="status" class="input-outlined w-full" :aria-label="pt('statusDraft')">
        <option value="draft">{{ pt('statusDraft') }}</option>
        <option value="published">{{ pt('statusPublished') }}</option>
      </select>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <div class="flex gap-2">
        <button type="submit" class="btn-filled">{{ pt('save') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </div>
    </form>
  </div>
</template>
