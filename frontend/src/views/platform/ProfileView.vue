<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAuthStore, type InstitutionType } from '@/stores/auth'
import { usePt, platformErrorMessage, type PlatformKey } from '@/i18n/platform'
import { updateProfile, getTeacher } from '@/api/platformLearning'

const BIO_MAX = 1000
const pt = usePt()
const auth = useAuthStore()
const fullName = ref(auth.profile?.full_name ?? '')
const bio = ref('')
/**
 * True only once the bio was really read from the server. If that request failed the field stays empty, and sending
 * that empty value back would silently erase the saved bio, so the field is locked and `bio` is left out of the save.
 */
const bioLoaded = ref(false)
const bioLoading = ref(false)
const institutionType = ref<InstitutionType | ''>(auth.profile?.institution_type ?? '')
const msg = ref('')
const error = ref('')
const saving = ref(false)

const isTeacher = computed(() => auth.role === 'teacher')
const hasType = computed(() => auth.role === 'student' || auth.role === 'teacher')
const roleKey: Record<string, PlatformKey> = {
  student: 'roleStudent', teacher: 'roleTeacher', moderator: 'roleModerator', institution_admin: 'roleInstitutionAdmin', admin: 'roleAdmin',
}
const typeKey: Record<InstitutionType, PlatformKey> = { school: 'typeSchool', institute: 'typeInstitute', university: 'typeUniversity' }
const TYPES: InstitutionType[] = ['school', 'institute', 'university']

async function loadBio() {
  if (!isTeacher.value || !auth.profile) return
  bioLoading.value = true
  try {
    bio.value = (await getTeacher(auth.profile.id)).bio ?? ''
    bioLoaded.value = true
  } catch {
    bioLoaded.value = false
  } finally {
    bioLoading.value = false
  }
}
onMounted(loadBio)

async function save() {
  msg.value = error.value = ''
  saving.value = true
  const typeChanged = hasType.value && institutionType.value !== '' && institutionType.value !== auth.profile?.institution_type
  try {
    await updateProfile({
      full_name: fullName.value,
      ...(isTeacher.value && bioLoaded.value ? { bio: bio.value } : {}),
      ...(typeChanged ? { institution_type: institutionType.value as InstitutionType } : {}),
    })
    if (auth.profile) {
      auth.profile.full_name = fullName.value.trim()
      if (typeChanged) auth.profile.institution_type = institutionType.value as InstitutionType
    }
    msg.value = pt('saved')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8" data-testid="profile-page">
    <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
    <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('myProfile') }}</h1>

    <dl class="card-filled p-4 mb-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-body-md" data-testid="profile-info">
      <dt class="font-semibold">{{ pt('email') }}</dt>
      <dd class="min-w-0 break-all" dir="ltr" data-testid="profile-email">{{ auth.profile?.email }}</dd>
      <dt class="font-semibold">{{ pt('accountRole') }}</dt>
      <dd data-testid="profile-role">{{ auth.role ? pt(roleKey[auth.role]!) : '' }}</dd>
    </dl>

    <router-link v-if="isTeacher && auth.profile" :to="`/platform/teachers/${auth.profile.id}`" class="btn-tonal mb-4" data-testid="profile-public-link">{{ pt('profViewPublic') }}</router-link>

    <form class="card-elevated p-5 space-y-4" @submit.prevent="save">
      <label class="block">
        <span class="text-label-lg font-semibold">{{ pt('fullName') }}</span>
        <input v-model="fullName" required maxlength="120" class="input-outlined mt-1 w-full" dir="auto" data-testid="profile-name" />
      </label>

      <label v-if="hasType" class="block">
        <span class="text-label-lg font-semibold">{{ pt('institutionType') }}</span>
        <select v-model="institutionType" class="input-outlined mt-1 w-full" aria-describedby="profile-type-hint" data-testid="profile-institution-type">
          <option v-if="!auth.profile?.institution_type" value="" disabled>—</option>
          <option v-for="t in TYPES" :key="t" :value="t">{{ pt(typeKey[t]) }}</option>
        </select>
        <span id="profile-type-hint" class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ pt(auth.role === 'student' ? 'profTypeHint' : 'profTypeHintTeacher') }}</span>
      </label>

      <div v-if="isTeacher" class="block">
        <label class="block" for="profile-bio">
          <span class="text-label-lg font-semibold">{{ pt('bio') }}</span>
        </label>
        <textarea
          id="profile-bio"
          v-model="bio"
          :maxlength="BIO_MAX"
          rows="5"
          dir="auto"
          :disabled="!bioLoaded"
          aria-describedby="profile-bio-count"
          class="input-outlined mt-1 w-full"
          data-testid="profile-bio"
        ></textarea>
        <div class="flex items-start justify-between gap-3 mt-1">
          <p v-if="!bioLoaded && !bioLoading" class="text-body-sm flex-1" role="alert" style="color: rgb(var(--md-error))" data-testid="profile-bio-failed">
            {{ pt('profBioFailed') }}
            <button type="button" class="underline font-semibold" data-testid="profile-bio-reload" @click="loadBio">{{ pt('profBioReload') }}</button>
          </p>
          <span v-else class="flex-1"></span>
          <span id="profile-bio-count" class="text-body-sm shrink-0" style="color: rgb(var(--md-on-surface-variant))" data-testid="profile-bio-count"><span dir="ltr" class="inline-block">{{ bio.length }} / {{ BIO_MAX }}</span></span>
        </div>
      </div>

      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status" data-testid="profile-saved">{{ msg }}</p>
      <button type="submit" class="btn-filled" :disabled="saving" data-testid="profile-save">{{ pt('save') }}</button>
    </form>
  </div>
</template>
