<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { usePt, platformErrorMessage, platformAdminErrorMessage, type PlatformKey } from '@/i18n/platform'
import { useI18nStore } from '@/stores/i18n'
import SubjectPicker from '@/components/platform/SubjectPicker.vue'
import { deleteExam, examAction, listExams, type ExamAction, type ExamRow, type Phase } from '@/api/platformExamAdmin'

const pt = usePt()
const i18n = useI18nStore()
const router = useRouter()
const PAGE = 25

const items = ref<ExamRow[]>([])
const total = ref(0)
const page = ref(0)
const loading = ref(false)
const error = ref('')
const notice = ref('')
const subjectId = ref('')
const search = ref('')
const filter = reactive({ status: '' as Phase | 'all' | '', q: '' })
const pages = computed(() => Math.max(1, Math.ceil(total.value / PAGE)))

async function load() {
  loading.value = true
  error.value = ''
  try {
    const r = await listExams({ subject_id: subjectId.value || undefined, status: filter.status || undefined, q: filter.q || undefined, limit: PAGE, offset: page.value * PAGE })
    items.value = r.items
    total.value = r.total
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    loading.value = false
  }
}
const refilter = () => { page.value = 0; load() }
watch(subjectId, refilter, { immediate: true })
function submitSearch() { filter.q = search.value; refilter() }

async function run(row: ExamRow, action: ExamAction) {
  error.value = ''
  notice.value = ''
  try {
    const d = await examAction(row.id, action)
    if (action === 'duplicate') { await router.push(`/platform/admin/exams/${d.id}/edit`); return }
    notice.value = pt('bankDone')
    await load()
  } catch (e) {
    error.value = platformAdminErrorMessage(pt, e)
  }
}

async function remove(row: ExamRow) {
  error.value = ''
  notice.value = ''
  let typed: string | undefined
  if (row.attempt_count > 0) {
    const t = window.prompt(pt('exDeleteTyped').replace('{n}', String(row.attempt_count)))
    if (t === null) return
    typed = t
  } else if (!window.confirm(pt('exDeleteConfirm'))) return
  try {
    await deleteExam(row.id, typed)
    notice.value = pt('bankDone')
    if (page.value > 0 && items.value.length === 1) page.value--
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

const fmt = (ms: number | null) => (ms ? new Date(ms).toLocaleString(i18n.locale === 'ar' ? 'ar' : undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '')
const tabs: { key: Phase | 'all' | ''; label: PlatformKey }[] = [
  { key: '', label: 'exAll' }, { key: 'draft', label: 'exPhase_draft' }, { key: 'published', label: 'exPhase_published' }, { key: 'closed', label: 'exPhase_closed' }, { key: 'archived', label: 'exPhase_archived' },
]
</script>

<template>
  <div class="max-w-4xl mx-auto pb-12 space-y-4" data-testid="exams-page">
    <div class="flex items-center gap-3 flex-wrap">
      <h1 class="text-display-sm font-bold tracking-tight flex-1">{{ pt('exTitle') }}</h1>
      <router-link to="/platform/admin/exams/new" class="btn-filled" data-testid="exam-new">{{ pt('exNew') }}</router-link>
    </div>

    <SubjectPicker v-model="subjectId" :remember="false" />
    <form class="flex gap-2" @submit.prevent="submitSearch">
      <input v-model="search" type="search" :placeholder="pt('exSearch')" class="input-outlined flex-1 min-w-0" data-testid="exam-search" />
    </form>
    <div class="flex flex-wrap gap-2">
      <button v-for="t in tabs" :key="t.key" :class="filter.status === t.key ? 'btn-filled' : 'btn-outlined'" :data-testid="'tab-' + (t.key || 'default')" @click="filter.status = t.key; refilter()">{{ pt(t.label) }}</button>
    </div>

    <p v-if="notice" role="status" class="text-body-md" data-testid="exam-notice">{{ notice }}</p>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="exam-error">{{ error }}</p>
    <p v-if="!loading && !items.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="exam-empty">{{ pt('exListEmpty') }}</p>

    <ul class="space-y-3">
      <li v-for="e in items" :key="e.id" class="card-filled p-4 space-y-2" data-testid="exam-row">
        <div class="flex items-start gap-3 flex-wrap">
          <div class="min-w-0 flex-1">
            <router-link :to="e.can_edit ? `/platform/admin/exams/${e.id}/edit` : `/platform/assessments/${e.id}`" class="font-bold break-words" data-testid="exam-title">{{ e.title }}</router-link>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              {{ e.subject_name }} · <span dir="ltr" class="inline-block">{{ e.question_count }}</span> {{ pt('exQuestions') }} · <span dir="ltr" class="inline-block">{{ e.total_points }}</span> {{ pt('exPoints') }} · <span dir="ltr" class="inline-block" data-testid="exam-attempts">{{ e.attempt_count }}</span> {{ pt('exAttemptsCount') }}
              · {{ pt('exBy') }} {{ e.teacher_name }}
            </div>
            <div v-if="e.opens_at || e.closes_at" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
              <template v-if="e.opens_at">{{ pt('exOpens').replace(/ \(.*\)/, '') }}: {{ fmt(e.opens_at) }}</template><template v-if="e.opens_at && e.closes_at"> · </template><template v-if="e.closes_at">{{ pt('exCloses').replace(/ \(.*\)/, '') }}: {{ fmt(e.closes_at) }}</template>
            </div>
            <div v-if="e.owned === false" class="text-body-sm font-semibold">{{ pt('exTeacherOwned') }}</div>
            <div v-if="e.locked" class="text-body-sm font-semibold" data-testid="exam-locked-note">{{ pt('exLockedByStaff') }}</div>
          </div>
          <span class="text-xs font-semibold px-2 py-1 rounded-full shrink-0" style="background-color: rgb(var(--md-surface-container-high))" data-testid="exam-phase">{{ pt(`exPhase_${e.phase}` as PlatformKey) }}</span>
        </div>
        <div class="flex flex-wrap gap-1">
          <router-link v-if="e.can_edit && e.status !== 'archived'" :to="`/platform/admin/exams/${e.id}/edit`" class="btn-text" data-testid="act-edit">{{ pt('exEdit') }}</router-link>
          <button v-if="e.can_edit && e.status === 'draft'" class="btn-text" data-testid="act-publish" @click="run(e, 'publish')">{{ pt('exPublish') }}</button>
          <button v-if="e.status === 'published' && e.attempt_count === 0" class="btn-text" data-testid="act-unpublish" @click="run(e, 'unpublish')">{{ pt('exUnpublish') }}</button>
          <button v-if="e.status === 'published'" class="btn-text" data-testid="act-close" @click="run(e, 'close')">{{ pt('exClose') }}</button>
          <button v-if="e.status === 'closed'" class="btn-text" data-testid="act-reopen" @click="run(e, 'reopen')">{{ pt('exReopen') }}</button>
          <button class="btn-text" data-testid="act-duplicate" @click="run(e, 'duplicate')">{{ pt('exDuplicate') }}</button>
          <button v-if="e.status === 'draft' || e.status === 'closed'" class="btn-text" data-testid="act-archive" @click="run(e, 'archive')">{{ pt('exArchive') }}</button>
          <button v-if="e.status === 'archived'" class="btn-text" data-testid="act-restore" @click="run(e, 'restore')">{{ pt('exRestore') }}</button>
          <button v-if="e.locked" class="btn-text" data-testid="act-unlock" @click="run(e, 'unlock')">{{ pt('exUnlock') }}</button>
          <router-link v-if="e.attempt_count > 0" :to="`/platform/assessments/${e.id}/results`" class="btn-text" data-testid="act-results">{{ pt('exResults') }}</router-link>
          <button class="btn-text" data-testid="act-delete" @click="remove(e)">{{ pt('exDelete') }}</button>
        </div>
      </li>
    </ul>

    <nav v-if="pages > 1" class="flex items-center justify-between gap-3" :aria-label="pt('pageLabel')" data-testid="exam-pager">
      <button class="btn-outlined" :disabled="page === 0 || loading" data-testid="exam-prev" @click="page--; load()">{{ pt('prevPage') }}</button>
      <span class="text-body-sm"><span dir="ltr" class="inline-block">{{ page + 1 }} / {{ pages }}</span> · <span dir="ltr" class="inline-block">{{ total }}</span> {{ pt('bankFound') }}</span>
      <button class="btn-outlined" :disabled="page + 1 >= pages || loading" data-testid="exam-next" @click="page++; load()">{{ pt('nextPage') }}</button>
    </nav>
  </div>
</template>
