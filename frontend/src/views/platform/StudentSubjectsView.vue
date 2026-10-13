<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { useStudentStats } from '@/lib/studentStats'
import { enroll, myEnrollments, unenroll, type EnrolledSubject } from '@/api/platformLearning'
import { listInstitutions, type Institution } from '@/api/platformAdmin'
import { BULK_ENROLL_MAX, bulkEnroll, suggestedSubjects, type SuggestedSubject, type SuggestedSubjects } from '@/api/platformStudent'
import { fillTemplate } from '@/utils/notificationText'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'
import ConfirmDialog from '@/components/platform/ConfirmDialog.vue'
import PageLoading from '@/components/platform/PageLoading.vue'

// "My subjects": the subjects I am enrolled in (a switched-off one stays listed, flagged, so I can still unenrol), the
// subjects of the level I placed myself at (one click each, or all at once), and the institutions to browse (choosing my
// level happens on an institution's page). Three independent requests: a section whose request failed says so, the
// others still show.
const pt = usePt()
const auth = useAuthStore()
const { refresh: refreshStats } = useStudentStats()

const loaded = ref(false)
const mine = ref<EnrolledSubject[]>([])
const mineError = ref('')
const suggest = ref<SuggestedSubjects | null>(null)
const suggestError = ref('')
const institutions = ref<Institution[]>([])
const instError = ref('')
const notice = ref('')
const error = ref('')
const busy = ref('')

const placement = computed(() => suggest.value?.placement ?? null)
const toEnroll = computed(() => (suggest.value?.subjects ?? []).filter((s) => !s.enrolled))
const placementText = computed(() => (placement.value ? [placement.value.institution_name, ...placement.value.unit_path].join(' ‹ ') : ''))

async function loadMine() {
  try {
    mine.value = await myEnrollments()
    mineError.value = ''
  } catch (e) {
    mineError.value = platformErrorMessage(pt, e)
  }
}
async function loadSuggest() {
  try {
    suggest.value = await suggestedSubjects()
    suggestError.value = ''
  } catch (e) {
    suggestError.value = platformErrorMessage(pt, e)
  }
}
async function loadInstitutions() {
  try {
    institutions.value = await listInstitutions(auth.profile?.institution_type ?? undefined)
    instError.value = ''
  } catch (e) {
    instError.value = platformErrorMessage(pt, e)
  }
}
onMounted(async () => {
  await Promise.allSettled([loadMine(), loadSuggest(), loadInstitutions()])
  loaded.value = true
})

/** After any enrolment change: both lists and the sidebar numbers. */
async function reloadAfterChange() {
  await Promise.allSettled([loadMine(), loadSuggest()])
  refreshStats()
}

const errorOf = (e: unknown) => (e instanceof PlatformError && e.code === 'invalid_selection' ? pt('errInvalidSelection') : platformErrorMessage(pt, e))

// ── one subject
async function enrollOne(s: SuggestedSubject) {
  if (busy.value) return
  busy.value = s.id
  error.value = notice.value = ''
  try {
    await enroll(s.id)
    notice.value = fillTemplate(pt('mysubjEnrolledOne'), { name: s.name_ar })
    await reloadAfterChange()
  } catch (e) {
    error.value = errorOf(e)
  } finally {
    busy.value = ''
  }
}

// ── all the subjects of my level (in requests of at most BULK_ENROLL_MAX)
const askingAll = ref(false)
const allBusy = ref(false)
const allError = ref('')
function askAll() {
  allError.value = ''
  askingAll.value = true
}
async function enrollAll() {
  if (allBusy.value) return
  const ids = toEnroll.value.map((s) => s.id)
  if (!ids.length) { askingAll.value = false; return }
  allBusy.value = true
  allError.value = ''
  let enrolled = 0
  let skipped = 0
  try {
    for (let i = 0; i < ids.length; i += BULK_ENROLL_MAX) {
      const r = await bulkEnroll(ids.slice(i, i + BULK_ENROLL_MAX))
      enrolled += r.enrolled
      skipped += r.skipped
    }
    askingAll.value = false
    notice.value = fillTemplate(pt(skipped ? 'mysubjEnrolledManySkipped' : 'mysubjEnrolledMany'), { n: enrolled, skipped })
    error.value = ''
    await reloadAfterChange()
  } catch (e) {
    allError.value = errorOf(e)
    if (enrolled) await reloadAfterChange() // an earlier chunk went through
  } finally {
    allBusy.value = false
  }
}

// ── unenrol (asked first: the subject's exams and content leave my lists)
const leaving = ref<EnrolledSubject | null>(null)
const leaveBusy = ref(false)
const leaveError = ref('')
function askLeave(s: EnrolledSubject) {
  leaveError.value = ''
  leaving.value = s
}
async function confirmLeave() {
  const s = leaving.value
  if (!s || leaveBusy.value) return
  leaveBusy.value = true
  leaveError.value = ''
  try {
    await unenroll(s.subject_id)
    leaving.value = null
    notice.value = fillTemplate(pt('mysubjUnenrolledOne'), { name: s.subject_name })
    error.value = ''
    await reloadAfterChange()
  } catch (e) {
    leaveError.value = errorOf(e)
  } finally {
    leaveBusy.value = false
  }
}

const isActive = (s: EnrolledSubject) => s.active !== false
</script>

<template>
  <div v-if="loaded" class="max-w-4xl mx-auto pb-12 space-y-6" data-testid="mysubj-page">
    <div>
      <h1 class="text-display-sm font-bold tracking-tight">{{ pt('mysubjTitle') }}</h1>
      <p class="text-body-md" style="color: rgb(var(--md-on-surface-variant))">{{ pt('mysubjSub') }}</p>
    </div>
    <p v-if="notice" role="status" class="text-body-md font-semibold" style="color: rgb(var(--azl-success-text, var(--md-primary)))" data-testid="mysubj-notice">{{ notice }}</p>
    <p v-if="error" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="mysubj-error">{{ error }}</p>

    <!-- my enrolments -->
    <section aria-labelledby="mysubj-mine-title" data-testid="mysubj-mine">
      <h2 id="mysubj-mine-title" class="text-title-md font-bold mb-3">{{ pt('mysubjMine') }}</h2>
      <p v-if="mineError" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="mysubj-mine-error">{{ mineError }}</p>
      <p v-else-if="!mine.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="mysubj-empty">{{ pt('noEnrollments') }}</p>
      <ul class="space-y-2">
        <li v-for="s in mine" :key="s.subject_id" class="card-filled p-4 flex flex-wrap items-center gap-3" :data-testid="`mysubj-row-${s.subject_id}`">
          <div class="min-w-0 flex-1">
            <router-link v-if="isActive(s)" :to="`/platform/subjects/${s.subject_id}`" class="font-bold break-words" dir="auto" :data-testid="`mysubj-link-${s.subject_id}`">{{ s.subject_name }}</router-link>
            <span v-else class="font-bold break-words" dir="auto">{{ s.subject_name }}</span>
            <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" dir="auto">{{ s.institution_name }}</div>
            <p v-if="!isActive(s)" class="text-body-sm mt-1" :data-testid="`mysubj-inactive-${s.subject_id}`">
              <span class="font-semibold px-2 py-0.5 rounded-full me-1" style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))">{{ pt('mysubjInactive') }}</span>
              {{ pt('mysubjInactiveWhy') }}
            </p>
          </div>
          <button type="button" class="btn-outlined" :aria-label="`${pt('unenroll')}: ${s.subject_name}`" :data-testid="`mysubj-unenroll-${s.subject_id}`" @click="askLeave(s)">{{ pt('unenroll') }}</button>
        </li>
      </ul>
    </section>

    <!-- the subjects of my level -->
    <section aria-labelledby="suggest-title" data-testid="suggest-section">
      <h2 id="suggest-title" class="text-title-md font-bold mb-1">{{ pt('suggestTitle') }}</h2>
      <p v-if="suggestError" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="suggest-error">{{ suggestError }}</p>
      <template v-else-if="placement">
        <p class="text-body-md mb-3">
          {{ pt('suggestYourLevel') }}: <bdi class="font-semibold" data-testid="suggest-placement">{{ placementText }}</bdi>
          · <router-link :to="`/platform/institutions/${placement.institution_id}`" class="underline" data-testid="suggest-change-level">{{ pt('suggestChangeLevel') }}</router-link>
        </p>
        <!-- the level was removed (or never chosen): nothing is suggested for the whole institution, the student chooses a level again -->
        <p v-if="!placement.unit_id" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="suggest-no-level">{{ pt('suggestNoLevel') }}</p>
        <p v-else-if="!suggest?.subjects.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))" data-testid="suggest-empty">{{ pt('suggestNone') }}</p>
        <template v-else>
          <button v-if="toEnroll.length" type="button" class="btn-filled mb-3" :disabled="!!busy" data-testid="suggest-enroll-all" @click="askAll">
            {{ pt('suggestEnrollAll') }} <span dir="ltr" class="inline-block">({{ toEnroll.length }})</span>
          </button>
          <p v-else class="text-body-md mb-3" data-testid="suggest-all-done">{{ pt('suggestAllDone') }}</p>
          <ul class="space-y-2">
            <li v-for="s in suggest!.subjects" :key="s.id" class="card-filled p-4 flex flex-wrap items-center gap-3" :data-testid="`suggest-row-${s.id}`">
              <div class="min-w-0 flex-1">
                <router-link :to="`/platform/subjects/${s.id}`" class="font-bold break-words" dir="auto">{{ s.name_ar }}</router-link>
                <div v-if="s.unit_path.length" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))"><bdi>{{ s.unit_path.join(' ‹ ') }}</bdi></div>
              </div>
              <span v-if="s.enrolled" class="px-3 py-1 rounded-full text-sm font-semibold" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" :data-testid="`suggest-enrolled-${s.id}`">{{ pt('enrolledBadge') }}</span>
              <button v-else type="button" class="btn-tonal" :disabled="!!busy" :aria-label="`${pt('enroll')}: ${s.name_ar}`" :data-testid="`suggest-enroll-${s.id}`" @click="enrollOne(s)">{{ pt('enroll') }}</button>
            </li>
          </ul>
        </template>
      </template>
      <p v-else class="text-body-md card-filled p-4" data-testid="suggest-no-placement">{{ pt('suggestNoPlacement') }}</p>
    </section>

    <!-- institutions (the old home's list): choose my level on an institution's page -->
    <section aria-labelledby="browse-title" data-testid="browse-institutions">
      <h2 id="browse-title" class="text-title-md font-bold mb-3">{{ pt('yourInstitutions') }}</h2>
      <p v-if="instError" role="alert" class="text-body-md" style="color: rgb(var(--md-error))" data-testid="browse-error">{{ instError }}</p>
      <p v-else-if="!institutions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noInstitutions') }}</p>
      <ul class="space-y-3">
        <li v-for="i in institutions" :key="i.id">
          <router-link :to="`/platform/institutions/${i.id}`" class="card-filled block p-4 no-underline" :data-testid="`browse-inst-${i.id}`">
            <span class="block font-bold" dir="auto">{{ i.name_ar }}</span>
            <span v-if="i.name_en || i.city" class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))" dir="auto">{{ [i.name_en, i.city].filter(Boolean).join(' · ') }}</span>
          </router-link>
        </li>
      </ul>
    </section>

    <ConfirmDialog
      v-if="askingAll"
      :title="pt('suggestEnrollAll')"
      :message="fillTemplate(pt('suggestConfirmMsg'), { n: toEnroll.length })"
      :items="toEnroll.map((s) => s.name_ar)"
      :confirm-label="pt('suggestConfirmYes')"
      :busy="allBusy"
      :error="allError"
      testid="suggest-confirm"
      @confirm="enrollAll"
      @close="askingAll = false"
    />
    <ConfirmDeleteDialog
      v-if="leaving"
      :title="`${pt('unenroll')}: ${leaving.subject_name}`"
      :message="pt('mysubjUnenrollMsg')"
      :confirm-label="pt('unenroll')"
      :busy="leaveBusy"
      :error="leaveError"
      @confirm="confirmLeave"
      @close="leaving = null"
    />
  </div>
  <PageLoading v-else />
</template>
