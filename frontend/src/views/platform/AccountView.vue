<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { PlatformError } from '@/lib/platformApi'
import { changePassword, deleteAccount, deletionImpact, type DeletionImpact } from '@/api/platformOps'
import { useSignOut } from '@/composables/useSignOut'
import ConfirmDeleteDialog from '@/components/platform/ConfirmDeleteDialog.vue'

const pt = usePt()
const auth = useAuthStore()
const signOut = useSignOut()

const current = ref('')
const next = ref('')
const msg = ref('')
const error = ref('')
const delPassword = ref('')
const delError = ref('')

// What deleting the account takes with it (a teacher's own exams, student attempts on them, courses, posts, live
// sessions; a student's exam attempts and results, completed lessons, enrolments and reviews). Read when the page opens
// and again right before the confirmation, so the dialog never shows old numbers.
const impact = ref<DeletionImpact | null>(null)
const impactKnown = ref(false)
const confirming = ref(false)
const busy = ref(false)
const dialogError = ref('')

const total = (i: DeletionImpact | null) =>
  i ? i.exams + i.attempts + i.courses + i.posts + i.live + (i.attempts_taken ?? 0) + (i.lessons_completed ?? 0) + (i.enrollments ?? 0) + (i.reviews ?? 0) : 0
// the dialog lists only the non-zero ones (a student's teacher figures are always 0, and the other way round)
const counts = computed(() => [
  { label: pt('delCntExams'), value: impact.value?.exams ?? 0 },
  { label: pt('delCntAttempts'), value: impact.value?.attempts ?? 0 },
  { label: pt('delCntCourses'), value: impact.value?.courses ?? 0 },
  { label: pt('delCntPosts'), value: impact.value?.posts ?? 0 },
  { label: pt('delCntLive'), value: impact.value?.live ?? 0 },
  { label: pt('delCntAttemptsTaken'), value: impact.value?.attempts_taken ?? 0 },
  { label: pt('delCntLessons'), value: impact.value?.lessons_completed ?? 0 },
  { label: pt('delCntEnrollments'), value: impact.value?.enrollments ?? 0 },
  { label: pt('delCntReviews'), value: impact.value?.reviews ?? 0 },
])
// Typing the e-mail is required whenever something of value goes with the account - a teacher's content, a student's
// exam results (the server insists on it then: 409 confirm_required) - and also when the numbers could not be read
// (then nothing is known to be safe to lose). Enrolments, completed lessons and reviews alone are listed but need only
// the password.
const teacherTotal = (i: DeletionImpact | null) => (i ? i.exams + i.attempts + i.courses + i.posts + i.live : 0)
const needsEmail = computed(() => !impactKnown.value || teacherTotal(impact.value) > 0 || (impact.value?.attempts_taken ?? 0) > 0)
// Students' attempts on this teacher's exams: the server refuses the deletion (409 has_attempts) and says so up front.
const blocked = computed(() => (impact.value?.attempts ?? 0) > 0)

async function loadImpact() {
  try {
    impact.value = await deletionImpact()
    impactKnown.value = true
  } catch {
    impact.value = null
    impactKnown.value = false
  }
}
onMounted(loadImpact)

async function savePassword() {
  msg.value = error.value = ''
  try {
    await changePassword(current.value, next.value)
    current.value = next.value = ''
    msg.value = pt('passwordChangedOk')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function askRemove() {
  delError.value = dialogError.value = ''
  await loadImpact()
  confirming.value = true
}

async function remove() {
  busy.value = true
  dialogError.value = ''
  try {
    // the dialog only lets this through once the typed address matches the account's (the server checks it again for a
    // student who has taken exams: 409 confirm_required)
    await deleteAccount(delPassword.value, needsEmail.value ? auth.profile?.email : undefined)
    confirming.value = false
    await signOut()
  } catch (e) {
    if (e instanceof PlatformError && e.code === 'confirm_required') {
      // exams were taken since the numbers were read: show them, and the dialog now asks for the e-mail
      dialogError.value = pt('delErrConfirmEmail')
      await loadImpact()
    } else {
      dialogError.value = e instanceof PlatformError && e.code === 'has_attempts' ? pt('delHasAttempts') : platformErrorMessage(pt, e)
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8 space-y-6">
    <div>
      <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
      <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('accountSettings') }}</h1>
      <p class="text-body-sm" dir="ltr" style="color: rgb(var(--md-on-surface-variant))">{{ auth.profile?.email }}</p>
    </div>

    <form class="card-elevated p-5 space-y-3" @submit.prevent="savePassword">
      <h2 class="font-bold">{{ pt('changePassword') }}</h2>
      <input v-model="current" type="password" required autocomplete="current-password" dir="ltr" :placeholder="pt('currentPassword')" :aria-label="pt('currentPassword')" class="input-outlined w-full" data-testid="pw-current" />
      <input v-model="next" type="password" required minlength="8" maxlength="128" autocomplete="new-password" dir="ltr" :placeholder="pt('newPassword')" :aria-label="pt('newPassword')" class="input-outlined w-full" data-testid="pw-new" />
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <button type="submit" class="btn-filled" data-testid="pw-save">{{ pt('save') }}</button>
    </form>

    <form class="card-filled p-5 space-y-3" @submit.prevent="askRemove">
      <h2 class="font-bold" style="color: rgb(var(--md-error))">{{ pt('deleteAccount') }}</h2>
      <p class="text-body-sm">{{ pt('deleteAccountHint') }}</p>
      <p v-if="auth.role === 'teacher'" class="text-body-sm" data-testid="del-teacher-hint">{{ pt('deleteAccountTeacherHint') }}</p>
      <p v-if="auth.role === 'student'" class="text-body-sm" data-testid="del-student-hint">{{ pt('deleteAccountStudentHint') }}</p>
      <p v-if="blocked" class="text-body-sm font-semibold" role="note" data-testid="del-blocked">{{ pt('delBlockedNotice') }}</p>
      <input v-model="delPassword" type="password" required autocomplete="current-password" dir="ltr" :placeholder="pt('deleteAccountConfirm')" :aria-label="pt('deleteAccountConfirm')" class="input-outlined w-full" data-testid="del-password" />
      <p v-if="delError" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ delError }}</p>
      <button type="submit" class="btn-outlined" data-testid="del-submit">{{ pt('deleteAccount') }}</button>
    </form>

    <ConfirmDeleteDialog
      v-if="confirming"
      :title="pt('deleteAccount')"
      :message="total(impact) > 0 ? pt('delImpactMsg') : pt('deleteAccountHint')"
      :counts="counts"
      :confirm-name="needsEmail ? auth.profile?.email : undefined"
      confirm-kind="email"
      :busy="busy"
      :error="dialogError"
      @confirm="remove"
      @close="confirming = false"
    />
  </div>
</template>
