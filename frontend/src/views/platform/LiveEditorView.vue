<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type MyTeaching } from '@/api/platformLearning'
import { createLive, updateLive, deleteLive, getLive, type Live } from '@/api/platformContent'
import { isHttpsUrl } from '@/utils/videoKind'
import { usableSubjects } from '@/utils/teachingCards'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import UnsavedChangesDialog from '@/components/platform/UnsavedChangesDialog.vue'
import HiddenWhy from '@/components/platform/HiddenWhy.vue'
import OwnerStateChip from '@/components/platform/OwnerStateChip.vue'

// A live session has no draft: it is announced to the subject's students when it is created. Only what the teacher
// changed is sent (so the title of a session that is already over can still be corrected without touching its time).
const pt = usePt()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string | undefined
const isEdit = computed(() => !!id)

const subjects = ref<MyTeaching[]>([])
const subjectId = ref('')
const title = ref('')
const description = ref('')
const startsAt = ref('') // datetime-local value
const duration = ref(60)
const joinUrl = ref('')
const cancelled = ref(false)
const ended = ref(false)
const current = ref<Live | null>(null)
const error = ref('')
const msg = ref('')
const busy = ref(false)
const loaded = ref(false)
// what the session had when it was opened: the time is sent only when it was changed
let originalStartsAt = ''
let originalDuration = 60

const toLocalInput = (ms: number) => {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60000)
  return d.toISOString().slice(0, 16)
}

const { asking: leaving, answer: answerLeave, markClean } = useUnsavedGuard(() => ({
  subjectId: subjectId.value, title: title.value, description: description.value,
  startsAt: startsAt.value, duration: duration.value, joinUrl: joinUrl.value,
}))

onMounted(async () => {
  try {
    const [mine, live] = await Promise.all([myTeaching(), id ? getLive(id) : Promise.resolve(null)])
    subjects.value = usableSubjects(mine).filter((t) => t.status === 'approved')
    if (live) {
      current.value = live
      subjectId.value = live.subject_id; title.value = live.title; description.value = live.description ?? ''
      startsAt.value = toLocalInput(live.starts_at); duration.value = live.duration_min; joinUrl.value = live.join_url
      cancelled.value = live.status === 'cancelled'
      ended.value = !!live.ended
      originalStartsAt = startsAt.value
      originalDuration = live.duration_min
    } else {
      const wanted = route.query.subject as string | undefined
      subjectId.value = subjects.value.find((s) => s.subject_id === wanted)?.subject_id ?? subjects.value[0]?.subject_id ?? ''
    }
    loaded.value = true
    markClean()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

async function save() {
  if (busy.value) return
  error.value = msg.value = ''
  if (!isHttpsUrl(joinUrl.value)) {
    error.value = pt('invalidUrl')
    return
  }
  const ts = new Date(startsAt.value).getTime()
  if (!Number.isFinite(ts)) {
    error.value = pt('invalidTime')
    return
  }
  busy.value = true
  try {
    if (id) {
      const timeChanged = startsAt.value !== originalStartsAt
      const patch: Parameters<typeof updateLive>[1] = { title: title.value, description: description.value, join_url: joinUrl.value.trim() }
      if (timeChanged) patch.starts_at = ts
      if (duration.value !== originalDuration) patch.duration_min = duration.value
      const l = await updateLive(id, patch)
      current.value = l
      originalStartsAt = toLocalInput(l.starts_at)
      originalDuration = l.duration_min
      ended.value = !!l.ended
      markClean()
      msg.value = pt('saved')
    } else {
      await createLive({ subject_id: subjectId.value, title: title.value, description: description.value || undefined, starts_at: ts, duration_min: duration.value, join_url: joinUrl.value.trim() })
      markClean() // before navigating: the guard must not ask about what was just saved
      await router.replace('/platform/my-content')
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}

async function cancelLive() {
  if (!id || busy.value) return
  error.value = msg.value = ''
  busy.value = true
  try {
    const l = await updateLive(id, { status: cancelled.value ? 'scheduled' : 'cancelled' })
    current.value = l // the PATCH reply is in the owner's shape (ended / visible / hidden_reason), like GET /live/{id}
    cancelled.value = l.status === 'cancelled'
    ended.value = !!l.ended
    msg.value = cancelled.value ? pt('edLiveCancelled') : pt('edLiveReactivated')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    busy.value = false
  }
}

// --- delete --------------------------------------------------------------------------------------------------------------
const deleting = ref(false)
const deleteBusy = ref(false)
const deleteError = ref('')
async function remove() {
  if (!id || deleteBusy.value) return
  deleteBusy.value = true
  deleteError.value = ''
  try {
    await deleteLive(id)
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
  <div class="max-w-2xl mx-auto pb-8" data-testid="live-editor">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newLive') }}</h1>
    <p v-if="loaded && !subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="editor-no-subjects">
      {{ pt('noApprovedSubjects') }} <router-link to="/platform/teaching" class="underline font-semibold" data-testid="editor-go-teaching">{{ pt('hubGoTeaching') }}</router-link>
    </p>
    <p v-else-if="!loaded && error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>

    <form v-else-if="loaded" class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <div v-if="current" class="space-y-1">
        <OwnerStateChip :row="{ status: current.status, visible: current.visible, hidden_reason: current.hidden_reason, ended: current.ended }" />
        <HiddenWhy v-if="current.status === 'scheduled' && !current.ended" :reason="current.hidden_reason" />
      </div>
      <p v-if="ended" class="text-body-sm" role="note" data-testid="live-ended-note">{{ pt('edLiveEnded') }}</p>
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full" data-testid="live-subject">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" dir="auto" class="input-outlined mt-1 w-full" data-testid="live-title" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="3" dir="auto" class="input-outlined mt-1 w-full" data-testid="live-description"></textarea>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('startsAt') }}</span>
        <input v-model="startsAt" required type="datetime-local" class="input-outlined mt-1 w-full" data-testid="live-starts" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('durationMin') }}</span>
        <input v-model.number="duration" required type="number" min="5" max="480" class="input-outlined mt-1 w-full" data-testid="live-duration" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('joinUrl') }}</span>
        <input v-model="joinUrl" required type="url" dir="ltr" pattern="https://.+" placeholder="https://meet.google.com/..." class="input-outlined mt-1 w-full" data-testid="live-url" />
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('edLiveHttpsOnly') }}</span>
      </label>
      <p v-if="!isEdit" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="live-announce-hint">{{ pt('edLiveAnnounceHint') }}</p>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))" data-testid="editor-error">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status" data-testid="editor-msg">{{ msg }}</p>
      <div class="flex flex-wrap gap-2">
        <button type="submit" class="btn-filled" :disabled="busy" data-testid="live-save">{{ isEdit ? pt('edSaveChanges') : pt('edScheduleLive') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" :disabled="busy" data-testid="live-cancel" @click="cancelLive">{{ cancelled ? pt('reactivate') : pt('cancelLive') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" :disabled="busy" data-testid="live-delete" @click="deleteError = ''; deleting = true">{{ pt('del') }}</button>
      </div>
    </form>

    <ConfirmDeleteDialog
      v-if="deleting"
      :title="`${pt('del')}: ${title}`"
      :message="cancelled || ended ? pt('edDeleteLiveMsg') : `${pt('edDeleteLiveMsg')} ${pt('edDeleteLiveUpcoming')}`"
      :busy="deleteBusy"
      :error="deleteError"
      @confirm="remove"
      @close="deleting = false"
    />
    <UnsavedChangesDialog v-if="leaving" @stay="answerLeave(false)" @leave="answerLeave(true)" />
  </div>
</template>
