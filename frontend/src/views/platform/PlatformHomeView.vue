<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { listInstitutions, type Institution } from '@/api/platformAdmin'
import AssessmentList from '@/components/platform/AssessmentList.vue'
import { availableAssessments, type AssessmentInfo } from '@/api/platformExams'
import ContentLists from '@/components/platform/ContentLists.vue'
import { feed, type Bundle } from '@/api/platformContent'
import { adminStats, type Stats } from '@/api/platformOps'
import { notifications, myProgress, type ProgressItem } from '@/api/platformEngage'
import { myEnrollments, type EnrolledSubject } from '@/api/platformLearning'

const pt = usePt()
const auth = useAuthStore()
const router = useRouter()

const institutions = ref<Institution[]>([])
const enrolled = ref<EnrolledSubject[]>([])
const feedBundle = ref<Bundle>({ posts: [], courses: [], live: [] })
const error = ref('')
const unread = ref(0)
const stats = ref<Stats | null>(null)
const searchQ = ref('')
const available = ref<AssessmentInfo[]>([])
const progress = ref<ProgressItem[]>([])

onMounted(async () => {
  try {
    unread.value = (await notifications()).unread
  } catch { /* the bell is optional */ }
  if (auth.role === 'admin') {
    try { stats.value = await adminStats() } catch { /* stats are optional */ }
    return
  }
  try {
    // Teachers/students see the institutions of the type they registered with.
    institutions.value = await listInstitutions(auth.profile?.institution_type ?? undefined)
    if (auth.role === 'student') {
      enrolled.value = await myEnrollments()
      feedBundle.value = await feed()
      progress.value = await myProgress()
      available.value = await availableAssessments()
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
    <div class="flex items-center gap-3 mb-1">
      <h1 class="text-display-sm font-bold tracking-tight flex-1">{{ pt('platformHome') }}</h1>
      <router-link to="/platform/notifications" class="btn-outlined relative" :aria-label="pt('notifications')">
        {{ pt('notifications') }}
        <span v-if="unread" data-testid="unread" class="ms-2 px-2 rounded-full text-xs font-bold" style="background-color: rgb(var(--md-primary)); color: rgb(var(--md-on-primary))">{{ unread }}</span>
      </router-link>
    </div>
    <p class="text-body-lg mb-6" style="color: rgb(var(--md-on-surface-variant))">
      {{ pt('welcome') }} {{ auth.profile?.full_name }}
    </p>

    <section v-if="auth.role === 'admin'" class="space-y-3 mb-6">
      <h2 class="text-title-md font-bold">{{ pt('adminPanel') }}</h2>
      <div v-if="stats" class="grid grid-cols-2 gap-2" data-testid="stats">
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statUsers') }}</div><div class="text-title-lg font-bold" data-testid="stat-users">{{ Object.values(stats.users).reduce((a, b) => a + b, 0) }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statPendingTeachers') }}</div><div class="text-title-lg font-bold" data-testid="stat-pending">{{ stats.pending_teachers }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statOpenReports') }}</div><div class="text-title-lg font-bold" data-testid="stat-reports">{{ stats.open_reports }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statSignups') }}</div><div class="text-title-lg font-bold">{{ stats.signups_7d }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statContent') }}</div><div class="text-title-lg font-bold">{{ stats.content.posts + stats.content.courses + stats.content.assessments }}</div></div>
        <div class="card-filled p-3"><div class="text-body-sm">{{ pt('statAttempts') }}</div><div class="text-title-lg font-bold">{{ stats.attempts_submitted }}</div></div>
      </div>
      <router-link to="/platform/admin/reports" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminReports') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminReportsDesc') }}</div>
      </router-link>
      <router-link to="/platform/admin/audit" class="card-filled block p-4">
        <div class="font-bold">{{ pt('adminAudit') }}</div>
        <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('adminAuditDesc') }}</div>
      </router-link>
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
      <form class="mb-4" @submit.prevent="router.push({ path: '/platform/search', query: { q: searchQ } })">
        <input v-model="searchQ" type="search" maxlength="60" :placeholder="pt('searchPlaceholder')" class="input-outlined w-full" data-testid="home-search" />
      </form>
      <nav class="flex flex-wrap gap-2 mb-6">
        <router-link to="/platform/teachers" class="btn-tonal">{{ pt('browseTeachers') }}</router-link>
        <router-link v-if="auth.role === 'teacher'" to="/platform/teaching" class="btn-tonal">{{ pt('myTeaching') }}</router-link>
        <router-link v-if="auth.role === 'teacher'" to="/platform/my-content" class="btn-filled">{{ pt('myContent') }}</router-link>
        <router-link to="/platform/profile" class="btn-outlined">{{ pt('myProfile') }}</router-link>
        <router-link to="/platform/account" class="btn-outlined">{{ pt('accountSettings') }}</router-link>
      </nav>

      <section v-if="available.length" class="mb-6">
        <h2 class="text-title-md font-bold mb-3">{{ pt('availableAssessments') }}</h2>
        <AssessmentList :items="available" />
      </section>

      <section v-if="progress.length" class="mb-6">
        <h2 class="text-title-md font-bold mb-3">{{ pt('continueLearning') }}</h2>
        <ul class="space-y-2">
          <li v-for="p in progress" :key="p.course_id">
            <router-link :to="`/platform/courses/${p.course_id}`" class="card-filled block p-3">
              <div class="font-bold">{{ p.title }}</div>
              <div class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ p.subject_name }} · <span dir="ltr" class="inline-block">{{ p.completed }}/{{ p.total }}</span></div>
              <div class="h-1.5 rounded-full overflow-hidden mt-2" style="background-color: rgb(var(--md-surface-container-high))">
                <div class="h-full" :style="{ width: (p.total ? (p.completed / p.total) * 100 : 0) + '%', backgroundColor: 'rgb(var(--md-primary))' }"></div>
              </div>
            </router-link>
          </li>
        </ul>
      </section>

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

    <div class="flex flex-wrap gap-2">
      <router-link v-if="auth.role === 'admin'" to="/platform/account" class="btn-outlined">{{ pt('accountSettings') }}</router-link>
      <button class="btn-outlined" @click="logout">{{ pt('logout') }}</button>
    </div>
  </div>
</template>
