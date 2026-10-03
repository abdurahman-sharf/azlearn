<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { myTeaching, type Teaching } from '@/api/platformLearning'
import { createLive, updateLive, deleteLive, myContent } from '@/api/platformContent'

const pt = usePt()
const route = useRoute()
const router = useRouter()
const id = route.params.id as string | undefined
const isEdit = computed(() => !!id)

const subjects = ref<Teaching[]>([])
const subjectId = ref('')
const title = ref('')
const description = ref('')
const startsAt = ref('') // datetime-local value
const duration = ref(60)
const joinUrl = ref('')
const cancelled = ref(false)
const error = ref('')
const msg = ref('')

const toLocalInput = (ms: number) => {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60000)
  return d.toISOString().slice(0, 16)
}

onMounted(async () => {
  try {
    subjects.value = (await myTeaching()).filter(t => t.status === 'approved')
    if (id) {
      const l = (await myContent()).live.find(x => x.id === id)
      if (l) {
        subjectId.value = l.subject_id; title.value = l.title; description.value = l.description ?? ''
        startsAt.value = toLocalInput(l.starts_at); duration.value = l.duration_min; joinUrl.value = l.join_url
        cancelled.value = l.status === 'cancelled'
      }
    } else {
      subjectId.value = (route.query.subject as string) || subjects.value[0]?.subject_id || ''
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

async function save() {
  error.value = msg.value = ''
  const ts = new Date(startsAt.value).getTime()
  try {
    if (id) {
      await updateLive(id, { title: title.value, description: description.value, starts_at: ts, duration_min: duration.value, join_url: joinUrl.value })
      msg.value = pt('saved')
    } else {
      await createLive({ subject_id: subjectId.value, title: title.value, description: description.value || undefined, starts_at: ts, duration_min: duration.value, join_url: joinUrl.value })
      router.replace('/platform/my-content')
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function cancelLive() {
  if (!id) return
  try {
    await updateLive(id, { status: cancelled.value ? 'scheduled' : 'cancelled' })
    cancelled.value = !cancelled.value
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove() {
  if (!id || !window.confirm(pt('confirmDelete'))) return
  try {
    await deleteLive(id)
    router.replace('/platform/my-content')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto pb-8">
    <router-link to="/platform/my-content" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ isEdit ? pt('edit') : pt('newLive') }}</h1>
    <p v-if="!subjects.length && !isEdit" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noApprovedSubjects') }}</p>

    <form v-else class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <label v-if="!isEdit" class="block">
        <span class="text-label-lg font-semibold">{{ pt('subject') }}</span>
        <select v-model="subjectId" required class="input-outlined mt-1 w-full">
          <option v-for="s in subjects" :key="s.subject_id" :value="s.subject_id">{{ s.subject_name }} — {{ s.institution_name }}</option>
        </select>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('title') }}</span>
        <input v-model="title" required maxlength="200" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('description') }}</span>
        <textarea v-model="description" maxlength="2000" rows="3" class="input-outlined mt-1 w-full"></textarea>
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('startsAt') }}</span>
        <input v-model="startsAt" required type="datetime-local" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('durationMin') }}</span>
        <input v-model.number="duration" required type="number" min="5" max="480" class="input-outlined mt-1 w-full" />
      </label>
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('joinUrl') }}</span>
        <input v-model="joinUrl" required type="url" dir="ltr" placeholder="https://meet.google.com/..." class="input-outlined mt-1 w-full" />
      </label>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <div class="flex flex-wrap gap-2">
        <button type="submit" class="btn-filled">{{ pt('save') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" @click="cancelLive">{{ cancelled ? pt('reactivate') : pt('cancelLive') }}</button>
        <button v-if="isEdit" type="button" class="btn-outlined" @click="remove">{{ pt('del') }}</button>
      </div>
    </form>
  </div>
</template>
