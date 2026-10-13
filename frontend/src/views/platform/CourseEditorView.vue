<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { createCourse, getCourse, updateCourse, deleteCourse, addLesson, updateLesson, deleteLesson, moveLesson, type Lesson, type PubStatus } from '@/api/platformContent'
import { classifyVideoUrl, embedsInPage, type VideoKind } from '@/utils/videoKind'
import { usableSubjects } from '@/utils/teachingCards'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'

// Draft first: a course is created as a draft, lessons are added on this screen, and "Publish" is offered once there is at
// least one lesson (the server refuses an empty course too). Lessons can be edited in place - their id, and so the
// students' completion marks, stay - and deleting one says what it costs.
const pt = usePt()
const i18n = useI18nStore()
const listSep = computed(() => (i18n.locale === 'ar' ? '، ' : ', '))
const route = useRoute()
const router = useRouter()
const id = ref(route.params.id as string | undefined)

const subjects = ref<MyTeaching[]>([])
const subjectId = ref('')
const title = ref('')
const description = ref('')
const status = ref<PubStatus>('draft')
const lessons = ref<Lesson[]>([])
const error = ref('')
const msg = ref('')
const busy = ref(false)
const loaded = ref(false)
const isEdit = computed(() => !!id.value)

// new-lesson form
const lTitle = ref('')
const lUrl = ref('')
const lSection = ref('')
const lDescription = ref('')
const lessonBusy = ref(false)

// inline edit of one lesson
interface LessonDraft { id: string; title: string; url: string; section: string; description: string }
const editing = ref<LessonDraft | null>(null)
const editBusy = ref(false)
const editError = ref('')
const rowBusy = ref('')

const KIND_LABEL: Record<VideoKind, PlatformKey> = { youtube: 'lessonKindYoutube', vimeo: 'lessonKindVimeo', file: 'lessonKindFile', link: 'lessonKindLink' }
/** The label next to a link field: where the video plays, or that the link is not acceptable. Empty while nothing is typed. */
function kindHint(url: string): { text: string; ok: boolean; embeds: boolean } | null {
  if (!url.trim()) return null
  const k = classifyVideoUrl(url)
  if (k === null) return { text: pt('lessonKindInvalid'), ok: false, embeds: false }
  return { text: pt(KIND_LABEL[k]), ok: true, embeds: embedsInPage(k) }
}
const newHint = computed(() => kindHint(lUrl.value))
const editHint = computed(() => (editing.value ? kindHint(editing.value.url) : null))
const sections = computed(() => [...new Set(lessons.value.map((l) => l.section).filter((s): s is string => !!s))])

const editChanged = computed(() => {
  const e = editing.value
  const orig = e ? lessons.value.find((l) => l.id === e.id) : undefined
  if (!e || !orig) return false
  return e.title !== orig.title || e.url !== orig.video_url || e.section !== (orig.section ?? '') || e.description !== (orig.description ?? '')
})
const { asking: leaving, answer: answerLeave, markClean } = useUnsavedGuard(() => ({
  subjectId: subjectId.value, title: title.value, description: description.value,
  lesson: [lTitle.value, lUrl.value, lDescription.value], editChanged: editChanged.value,
}))

const missingToPublish = computed(() => {
  const out: string[] = []
  if (!title.value.trim()) out.push(pt('edMissingTitle'))
  if (!lessons.value.length) out.push(pt('edMissingLesson'))
  return out
})

// The course's own fields as the server last had them (loaded or saved). A lesson action must never touch the typed
// title/description, nor count them as saved: they are the teacher's unsaved work until "Save" is pressed.
const saved = { subjectId: '', title: '', description: '' }
const rememberSaved = () => { saved.subjectId = subjectId.value; saved.title = title.value; saved.description = description.value }
const courseDirty = () => subjectId.value !== saved.subjectId || title.value !== saved.title || description.value !== saved.description

/** Opening the page: fills the whole form from the server. */
async function load() {
  if (!id.value) return
  const c = await getCourse(id.value)
  subjectId.value = c.subject_id; title.value = c.title; description.value = c.description ?? ''
  status.value = c.status; lessons.value = c.lessons
  rememberSaved()
}

/** After a lesson action: refreshes ONLY the lessons (and the status), so unsaved title/description edits survive. */
async function refreshLessons() {
  if (!id.value) return
  const c = await getCourse(id.value)
  lessons.value = c.lessons
  status.value = c.status
}

/**
 * A lesson action empties its own input (or closes its editor), which would look like a change to the guard: take the new
 * state as the saved one - but only while the course fields themselves are unchanged, or their warning would be lost.
 */
function settleGuard() {
  if (!courseDirty()) markClean()
}

onMounted(async () => {
  try {
    subjects.value = usableSubjects(await myTeaching()).filter((t) => t.status === 'approved')
    if (id.value) await load()
    else {
      const wanted = route.query.subject as string | undefined
      subjectId.value = subjects.value.find((s) => s.subject_id === wanted)?.subject_id ?? subjects.value[0]?.subject_id ?? ''
    }
    loaded.value = true
    markClean()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

/** Runs a server action with the page's error handling. Returns false when it failed (the message is on the page). */
async function run(fn: () => Promise<unknown>): Promise<boolean> {
  error.value = msg.value = ''
  try {
    await fn()
    return true
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
    return false
  }
}

/** Saves the course's own fields. `next` = the status to end up in (omitted: keep it; a new course is always a draft). */
async function save(next?: PubStatus) {
  if (busy.value) return
  busy.value = true
  try {
    await run(async () => {
      if (id.value) {
        const c = await updateCourse(id.value, { title: title.value, description: description.value, status: next ?? status.value })
        status.value = c.status
        rememberSaved()
        markClean()
        msg.value = next === 'published' ? pt('edPublished') : next === 'draft' ? pt('edUnpublished') : pt('saved')
      } else {
        const c = await createCourse({ subject_id: subjectId.value, title: title.value, description: description.value || undefined, status: 'draft' })
        id.value = c.id
        status.value = c.status
        lessons.value = c.lessons ?? []
        rememberSaved()
        markClean() // before navigating: the guard must not ask about what was just saved
        await router.replace(`/platform/courses/${c.id}/edit`)
        msg.value = pt('edCourseDraftSaved')
      }
    })
  } finally {
    busy.value = false
  }
}

async function addL() {
  if (lessonBusy.value || !id.value) return
  if (!classifyVideoUrl(lUrl.value)) {
    error.value = pt('invalidUrl')
    return
  }
  lessonBusy.value = true
  try {
    const ok = await run(async () => {
      await addLesson(id.value!, { title: lTitle.value, video_url: lUrl.value.trim(), section: lSection.value.trim() || undefined, description: lDescription.value.trim() || undefined })
      lTitle.value = lUrl.value = lDescription.value = '' // the section stays: lessons usually come in runs of one section
      await refreshLessons()
    })
    if (ok) {
      msg.value = pt('edLessonAdded')
      settleGuard()
    }
  } finally {
    lessonBusy.value = false
  }
}

const move = async (l: Lesson, d: 'up' | 'down') => {
  if (rowBusy.value) return
  rowBusy.value = l.id
  try {
    await run(async () => { await moveLesson(l.id, d); await refreshLessons() })
  } finally {
    rowBusy.value = ''
  }
}

function startEdit(l: Lesson) {
  editError.value = ''
  editing.value = { id: l.id, title: l.title, url: l.video_url, section: l.section ?? '', description: l.description ?? '' }
}
function cancelEdit() {
  editing.value = null
  editError.value = ''
}
async function saveEdit() {
  const e = editing.value
  if (!e || editBusy.value) return
  if (!classifyVideoUrl(e.url)) {
    editError.value = pt('invalidUrl')
    return
  }
  editBusy.value = true
  editError.value = ''
  try {
    await updateLesson(e.id, { title: e.title, video_url: e.url.trim(), section: e.section.trim(), description: e.description.trim() })
    editing.value = null
    msg.value = pt('edLessonSaved')
    error.value = ''
    await refreshLessons()
    settleGuard()
  } catch (err) {
    editError.value = platformErrorMessage(pt, err)
  } finally {
    editBusy.value = false
  }
}

// --- delete a lesson / the course ---------------------------------------------------------------------------------------
const delLesson = ref<Lesson | null>(null)
const delLessonBusy = ref(false)
const delLessonError = ref('')
async function confirmDelLesson() {
  const l = delLesson.value
  if (!l || delLessonBusy.value) return
  delLessonBusy.value = true
  delLessonError.value = ''
  try {
    await deleteLesson(l.id)
    delLesson.value = null
    if (editing.value?.id === l.id) editing.value = null
    await refreshLessons()
    settleGuard()
    msg.value = pt('edLessonDeleted')
  } catch (e) {
    delLessonError.value = platformErrorMessage(pt, e)
  } finally {
    delLessonBusy.value = false
  }
}

const deleting = ref(false)
const deleteBusy = ref(false)
const deleteError = ref('')
async function confirmDelCourse() {
  if (!id.value || deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await deleteCourse(id.value)
    markClean() // leaving on purpose
    deleting.value = false
    await router.replace('/platform/my-content')
  } catch (e) {
    deleteError.value = platformErrorMessage(pt, e)
  } finally {
    deleteBusy.value = false
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8" data-testid="course-editor">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newCourse') }}</h1>
    <p v-if="loaded && !subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="editor-no-subjects">
      {{ pt('noApprovedSubjects') }} <router-link to="/platform/teaching" class="underline font-semibold" data-testid="editor-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </p>
    <p v-else-if="!loaded && error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>

    <form v-else-if="loaded" class="card-elevated p-5 space-y-4" @submit.prevent="save()">
      <p v-if="isEdit" class="text-body-sm" data-testid="course-status">
        <span class="font-semibold">{{ pt('edStatus') }}:</span> {{ status === 'published' ? pt('statusPublished') : pt('statusDraft') }}
      </p>
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full" data-testid="course-subject">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="course-title" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="3" dir="auto" class="input-outlined mt-1 w-full" data-testid="course-description"></textarea>
      </label>
      <p v-if="!isEdit" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="course-draft-hint">{{ pt('edNewCourseHint') }}</p>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status" data-testid="editor-msg">{{ msg }}</p>
      <p v-if="isEdit && status === 'draft' && missingToPublish.length" id="course-publish-missing" class="text-body-sm" role="note" data-testid="course-publish-missing">{{ pt('edPublishNeeds') }} {{ missingToPublish.join(listSep) }}</p>
      <div class="flex flex-wrap gap-2">
        <button type="submit" class="btn-filled" :disabled="busy" data-testid="course-save">{{ isEdit && status === 'published' ? pt('edSaveChanges') : pt('edSaveDraft') }}</button>
        <button v-if="isEdit && status === 'draft'" type="button" class="btn-tonal" :disabled="busy || missingToPublish.length > 0" :aria-describedby="missingToPublish.length ? 'course-publish-missing' : undefined" data-testid="course-publish" @click="save('published')">{{ pt('publish') }}</button>
        <button v-if="isEdit && status === 'published'" type="button" class="btn-outlined" :disabled="busy" data-testid="course-unpublish" @click="save('draft')">{{ pt('unpublish') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" :disabled="busy" data-testid="course-delete" @click="deleteError = ''; deleting = true">{{ pt('del') }}</button>
      </div>
    </form>

    <section v-if="isEdit && loaded" class="mt-6" aria-labelledby="lessons-title">
      <h2 id="lessons-title" class="text-title-md font-bold mb-3">{{ pt('lessons') }} <span dir="ltr" class="inline-block text-body-sm font-normal" data-testid="lesson-count">({{ lessons.length }})</span></h2>
      <p v-if="!lessons.length" class="text-body-md mb-3" style="color: rgb(var(--md-on-surface-variant))" data-testid="lessons-empty">{{ pt('edNoLessons') }}</p>
      <ol class="space-y-2 mb-4" data-testid="lesson-list">
        <li v-for="(l, i) in lessons" :key="l.id" class="card-filled p-3" :data-testid="`lesson-${l.id}`">
          <template v-if="editing && editing.id === l.id">
            <form class="space-y-3" :aria-label="`${pt('edit')}: ${l.title}`" :data-testid="`lesson-edit-form-${l.id}`" @submit.prevent="saveEdit">
              <label class="block">
                <span class="text-label-lg font-semibold">{{ pt('lessonTitle') }}</span>
                <input v-model="editing.title" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="lesson-edit-title" />
              </label>
              <label class="block">
                <span class="text-label-lg font-semibold">{{ pt('videoUrl') }}</span>
                <input v-model="editing.url" required type="url" dir="ltr" class="input-outlined mt-1 w-full" data-testid="lesson-edit-url" />
              </label>
              <p v-if="editHint" class="text-body-sm font-semibold" :role="editHint.ok ? 'status' : 'alert'" :style="editHint.ok ? '' : 'color: rgb(var(--md-error))'" data-testid="lesson-edit-kind">{{ editHint.text }}</p>
              <label class="block">
                <span class="text-label-lg font-semibold">{{ pt('section') }}</span>
                <input v-model="editing.section" maxlength="120" dir="auto" list="lesson-sections" class="input-outlined mt-1 w-full" data-testid="lesson-edit-section" />
              </label>
              <label class="block">
                <span class="text-label-lg font-semibold">{{ pt('lessonDescription') }}</span>
                <textarea v-model="editing.description" maxlength="2000" rows="2" dir="auto" class="input-outlined mt-1 w-full" data-testid="lesson-edit-description"></textarea>
              </label>
              <p v-if="editError" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="lesson-edit-error">{{ editError }}</p>
              <div class="flex gap-2">
                <button type="submit" class="btn-filled" :disabled="editBusy" data-testid="lesson-edit-save">{{ pt('save') }}</button>
                <button type="button" class="btn-outlined" :disabled="editBusy" data-testid="lesson-edit-cancel" @click="cancelEdit">{{ pt('cancel') }}</button>
              </div>
            </form>
          </template>
          <template v-else>
            <div class="flex items-center gap-2">
              <span class="font-bold"><span dir="ltr" class="inline-block">{{ i + 1 }}.</span></span>
              <span class="flex-1 min-w-0 break-words" dir="auto">{{ l.title }}<span v-if="l.section" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"> · {{ l.section }}</span></span>
              <span class="text-xs font-semibold px-2 py-0.5 rounded-full shrink-0" style="background-color: rgb(var(--md-surface-container-high))" :data-testid="`lesson-kind-${l.id}`">{{ pt(KIND_LABEL[l.video_kind]) }}</span>
            </div>
            <div class="flex flex-wrap gap-x-1 mt-1">
              <button type="button" class="btn-text" :disabled="!!rowBusy || !!editing" :aria-label="`${pt('edit')}: ${l.title}`" :data-testid="`lesson-edit-${l.id}`" @click="startEdit(l)">{{ pt('edit') }}</button>
              <button type="button" class="btn-text" :disabled="i === 0 || !!rowBusy" :aria-label="`${pt('moveUp')}: ${l.title}`" :data-testid="`lesson-up-${l.id}`" @click="move(l, 'up')">{{ pt('moveUp') }}</button>
              <button type="button" class="btn-text" :disabled="i === lessons.length - 1 || !!rowBusy" :aria-label="`${pt('moveDown')}: ${l.title}`" :data-testid="`lesson-down-${l.id}`" @click="move(l, 'down')">{{ pt('moveDown') }}</button>
              <button type="button" class="btn-text" :disabled="!!rowBusy" :aria-label="`${pt('del')}: ${l.title}`" :data-testid="`lesson-delete-${l.id}`" @click="delLessonError = ''; delLesson = l">{{ pt('del') }}</button>
            </div>
          </template>
        </li>
      </ol>

      <form class="card-elevated p-4 space-y-3" :aria-label="pt('addLesson')" data-testid="lesson-add-form" @submit.prevent="addL">
        <h3 class="font-semibold">{{ pt('addLesson') }}</h3>
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('lessonTitle') }}</span>
          <input v-model="lTitle" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="lesson-title" />
        </label>
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('videoUrl') }}</span>
          <input v-model="lUrl" required type="url" dir="ltr" placeholder="https://" class="input-outlined mt-1 w-full" data-testid="lesson-url" />
        </label>
        <p v-if="newHint" class="text-body-sm font-semibold" :role="newHint.ok ? 'status' : 'alert'" :style="newHint.ok ? '' : 'color: rgb(var(--md-error))'" data-testid="lesson-kind-hint">{{ newHint.text }}</p>
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('section') }}</span>
          <input v-model="lSection" maxlength="120" dir="auto" list="lesson-sections" class="input-outlined mt-1 w-full" data-testid="lesson-section" />
        </label>
        <label class="block">
          <span class="text-label-lg font-semibold">{{ pt('lessonDescription') }}</span>
          <textarea v-model="lDescription" maxlength="2000" rows="2" dir="auto" class="input-outlined mt-1 w-full" data-testid="lesson-description"></textarea>
        </label>
        <button type="submit" class="btn-tonal" :disabled="lessonBusy" data-testid="lesson-add">{{ pt('add') }}</button>
      </form>
      <datalist id="lesson-sections">
        <option v-for="s in sections" :key="s" :value="s"></option>
      </datalist>
    </section>

    <ConfirmDeleteDialog
      v-if="delLesson"
      :title="`${pt('del')}: ${delLesson.title}`"
      :message="pt('edDeleteLessonMsg')"
      :busy="delLessonBusy"
      :error="delLessonError"
      @confirm="confirmDelLesson"
      @close="delLesson = null"
    />
    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('del')}: ${title}`"
      :message="status === 'published' ? `${pt('edDeleteCourseMsg')} ${pt('edDeletePublished')}` : pt('edDeleteCourseMsg')"
      :counts="[{ label: pt('lessonsCount'), value: lessons.length }]"
      :kept="pt('edDeleteCourseKept')"
      :confirm-name="status === 'published' ? title : undefined"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="confirmDelCourse"
      @close="deleting = false"
    />
    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
  </div>
</template>
