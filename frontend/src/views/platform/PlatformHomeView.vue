<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { listInstitutions, type Institution } from '@/api/platformAdmin'
import ContentLists from '@/components/platform/ContentLists.vue'
import { feed, type Bundle } from '@/api/platformContent'
import { myEnrollments, type EnrolledSubject } from '@/api/platformLearning'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()

const institutions = ref<Institution[]>([])
const enrolled = ref<EnrolledSubject[]>([])
const feedBundle = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')

onMounted(async () => {
  if (auth.role === 'admin') return
  try {
    // Teachers/students see the institutions of the type they registered with.
    institutions.value = await listInstitutions(auth.profile?.institution_type ?? undefined)
    if (auth.role === 'student') {
      enrolled.value = await myEnrollments()
      feedBundle.value = await feed()
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
})

async function logout() {
  await auth.signOut()
  router.replace('/mine')
}
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <h1 class="text-display-sm font-bold tracking-tight mb-1">{{ pt('platformHome') }}</h1>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('welcome') }} {{ auth.profile?.full_name }}
    </p>

    <section v-if="auth.role === 'admin'" class="space-y-3 mb-6">
      <h2 class="text-title-md font-bold">{{ pt('adminPanel') }}</h2>
      <router-link to="/platform/admin/users" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminUsers') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminUsersDesc') }}</div>
      </router-link>
      <router-link to="/platform/admin/teaching" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminTeaching') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminTeachingDesc') }}</div>
      </router-link>
      <router-link to="/platform/admin/institutions" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminInstitutions') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminInstitutionsDesc') }}</div>
      </router-link>
    </section>

    <template v-else>
      <nav class="flex flex-wrap gap-2 mb-6">
        <router-link to="/platform/teachers" class="btn-tonal">{{ pt('browseTeachers') }}</router-link>
        <router-link v-if="auth.role === 'teacher'" to="/platform/teaching" class="btn-tonal">{{ pt('myTeaching') }}</router-link>
        <router-link v-if="auth.role === 'teacher'" to="/platform/my-content" class="btn-filled">{{ pt('myContent') }}</router-link>
        <router-link to="/platform/profile" class="btn-outlined">{{ pt('myProfile') }}</router-link>
      </nav>

      <section v-if="auth.role === 'student' && (feedBundle.posts.length || feedBundle.courses.length || feedBundle.live.length)" class="mb-6">
        <h2 class="text-title-md font-bold mb-3">{{ pt('latestContent') }}</h2>
        <ContentLists :bundle="feedBundle" />
      </section>

      <section v-if="auth.role === 'student'" class="mb-6">
        <h2 class="text-title-md font-bold mb-3">{{ pt('mySubjects') }}</h2>
        <p v-if="!enrolled.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noEnrollments') }}</p>
        <ul class="space-y-2">
          <li v-for="s in enrolled" :key="s.subject_id">
            <router-link :to="`/platform/subjects/${s.subject_id}`" class="card-filled block p-3">
              <div class="font-bold">{{ s.subject_name }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ s.institution_name }}</div>
            </router-link>
          </li>
        </ul>
      </section>

      <section class="mb-6">
        <h2 class="text-title-md font-bold mb-3">{{ pt('yourInstitutions') }}</h2>
        <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
        <p v-else-if="!institutions.length" class="text-body-lg" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noInstitutions') }}</p>
        <ul class="space-y-3">
          <li v-for="i in institutions" :key="i.id">
            <router-link :to="`/platform/institutions/${i.id}`" class="card-filled block p-4">
              <div class="font-bold">{{ i.name_ar }}</div>
              <div v-if="i.name_en || i.city" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ [i.name_en, i.city].filter(Boolean).join(' · ') }}</div>
            </router-link>
          </li>
        </ul>
      </section>
    </template>

    <button class="btn-outlined" @click="logout">{{ pt('logout') }}</button>
  </div>
</template>
