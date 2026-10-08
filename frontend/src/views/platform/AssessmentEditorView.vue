<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePracticeStore } from '@/stores/practice'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { useTeacherStats } from '@/lib/teacherStats'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { createAssessment, updateAssessment, deleteAssessment, getAssessment } from '@/api/platformExams'
import { usableSubjects } from '@/utils/teachingCards'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const practice = usePracticeStore()
const id = route.params.id as string | undefined
const isEdit = computed(() => !!id)

const subjects = ref<MyTeaching[]>([])
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
const saving = ref(false)
const questionCount = ref(0)
/** the status the exam had when it was opened (a published exam keeps working while the admin's switch is off) */
const originalStatus = ref<'draft' | 'published' | 'closed' | 'archived'>('draft')
const removing = ref(false)
const removeBusy = ref(false)
const removeError = ref('')
const loaded = ref(false)

const { asking: leaving, answer: answerLeave, markClean } = useUnsavedGuard(() => ({
  subjectId: subjectId.value, bankId: bankId.value, title: title.value, description: description.value, duration: duration.value,
  opensAt: opensAt.value, closesAt: closesAt.value, maxAttempts: maxAttempts.value, showAnswers: showAnswers.value, status: status.value,
}))

// The admin can switch exam creation off: a new exam cannot be saved and a draft cannot be published meanwhile (the
// server enforces it with 403 exams_disabled; this only says so before the teacher fills in the form).
const { stats, refresh } = useTeacherStats()
const examsOff = computed(() => stats.value?.can_create_exams === false)
const createBlocked = computed(() => !isEdit.value && examsOff.value)
const publishBlocked = computed(() => examsOff.value && originalStatus.value !== 'published')

const bank = computed(() => practice.banks.find(b => b.id === bankId.value))
const toMs = (v: string) => (v ? new Date(v).getTime() : undefined)
const toLocal = (ms: number | null) => (ms ? new Date(ms - new Date(ms).getTimezoneOffset() * 60000).toISOString().slice(0, 16) : '')

onMounted(async () => {
  refresh()
  try {
    subjects.value = usableSubjects(await myTeaching()).filter(t => t.status === 'approved')
    if (id) {
      const a = await getAssessment(id)
      subjectId.value = a.subject_id; title.value = a.title; description.value = a.description ?? ''
      duration.value = a.duration_min; opensAt.value = toLocal(a.opens_at); closesAt.value = toLocal(a.closes_at)
      maxAttempts.value = a.max_attempts; showAnswers.value = a.show_answers; status.value = a.status === 'published' ? 'published' : 'draft'
      originalStatus.value = a.status
      questionCount.value = a.question_count
      locked.value = a.attempt_count > 0
    } else {
      const wanted = route.query.subject as string | undefined
      subjectId.value = subjects.value.find(s => s.subject_id === wanted)?.subject_id ?? subjects.value[0]?.subject_id ?? ''
      bankId.value = practice.banks[0]?.id ?? ''
      title.value = practice.banks[0]?.name ?? ''
    }
    loaded.value = true
    markClean()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

function onBank() {
  if (!title.value || practice.banks.some(b => b.name === title.value)) title.value = bank.value?.name ?? ''
}

async function save() {
  error.value = msg.value = ''
  if (createBlocked.value || (publishBlocked.value && status.value === 'published')) { error.value = pt('errExamsDisabled'); return }
  if (saving.value) return
  const common = {
    title: title.value, description: description.value || undefined,
    duration_min: duration.value || undefined, opens_at: toMs(opensAt.value), closes_at: toMs(closesAt.value),
    max_attempts: maxAttempts.value, show_answers: showAnswers.value, status: status.value,
  }
  saving.value = true
  try {
    if (id) {
      await updateAssessment(id, {
        ...common,
        clear_duration: !duration.value || undefined,
        clear_window: (!opensAt.value && !closesAt.value) || undefined,
      })
      originalStatus.value = status.value
      markClean()
      msg.value = pt('saved')
    } else {
      if (!bank.value) return
      const a = await createAssessment({ ...common, subject_id: subjectId.value, questions: bank.value.questions })
      markClean() // before navigating: the guard must not ask about what was just saved
      await router.replace(`/platform/assessments/${a.id}`)
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    if (e instanceof PlatformError && e.code === 'exams_disabled') refresh() // the numbers were stale: show the notice
  } finally {
    saving.value = false
  }
}

function askRemove() {
  removeError.value = ''
  removing.value = true
}

async function remove() {
  if (!id) return
  removeBusy.value = true
  removeError.value = ''
  try {
    await deleteAssessment(id)
    markClean() // leaving on purpose
    removing.value = false
    await router.replace('/platform/my-content')
  } catch (e) {
    // 409 has_attempts: students took it meanwhile; the delete is refused (it would erase their results)
    removeError.value = e instanceof PlatformError && e.code === 'has_attempts' ? pt('exDeleteBlocked') : platformErrorMessage(pt, e)
    if (e instanceof PlatformError && e.code === 'has_attempts') locked.value = true
  } finally {
    removeBusy.value = false
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newAssessment') }}</h1>
    <p v-if="createBlocked" class="card-filled p-3 mb-3 text-body-md" role="status" data-testid="exams-off-create">{{ pt('exDisabledCreate') }}</p>
    <p v-else-if="isEdit && publishBlocked" class="card-filled p-3 mb-3 text-body-md" role="status" data-testid="exams-off-publish">{{ pt('exDisabledPublish') }}</p>
    <p v-if="loaded && !subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="editor-no-subjects">
      {{ pt('noApprovedSubjects') }} <router-link to="/platform/teaching" class="underline font-semibold" data-testid="editor-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </p>
    <p v-else-if="!loaded && error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
    <p v-else-if="loaded && !isEdit && !practice.banks.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('noBanks') }} <router-link to="/generate" class="underline">/generate</router-link>
    </p>

    <form v-else-if="loaded" class="card-elevated p-5 space-y-4" @submit.prevent="save">
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
          <!-- the banks are not part of the account: they live in this browser only -->
          <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))" data-testid="bank-local-note">{{ pt('edBankLocalNote') }}</span>
        </label>
      </template>
      <p v-else-if="locked" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('hasAttempts') }}</p>

      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="assessment-title" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="2" dir="auto" class="input-outlined mt-1 w-full"></textarea>
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
      <select v-model="status" class="input-outlined w-full" :aria-label="pt('statusDraft')" data-testid="assessment-status">
        <option value="draft" :disabled="locked && originalStatus === 'published'">{{ pt('statusDraft') }}</option>
        <option value="published" :disabled="publishBlocked">{{ pt('statusPublished') }}</option>
      </select>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <p v-if="isEdit && locked" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="delete-blocked">{{ pt('exDeleteBlocked') }}</p>
      <div class="flex gap-2">
        <button type="submit" class="btn-filled" :disabled="saving || createBlocked" data-testid="assessment-save">{{ pt('save') }}</button>
        <!-- an exam that students took cannot be deleted (it would erase their results): no button, the reason above -->
        <button v-if="isEdit && !locked" type="button" class="btn-outlined" data-testid="assessment-delete" @click="askRemove">{{ pt('del') }}</button>
      </div>
    </form>

    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
    <ConfirmDeleteDialog
      v-if="removing"
      :title="`${pt('del')}: ${title}`"
      :message="pt('exDeleteMsg')"
      :counts="[{ label: pt('questionsCount'), value: questionCount }]"
      :busy="removeBusy"
      :error="removeError"
      @confirm="remove"
      @close="removing = false"
    />
  </div>
</template>
