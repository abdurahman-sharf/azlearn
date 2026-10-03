import { createRouter, createWebHashHistory } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { platformEnabled } from '@/lib/platformApi'

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: '/',
      redirect: '/practice',
    },
    {
      path: '/practice',
      name: 'practice',
      component: () => import('@/views/PracticeView.vue'),
      meta: { title: 'Practice' },
    },
    {
      path: '/generate',
      name: 'generate',
      component: () => import('@/views/GenerateView.vue'),
      meta: { title: 'Generate Exam' },
    },
    {
      path: '/search',
      name: 'search',
      component: () => import('@/views/SearchHomeView.vue'),
      meta: { title: 'Search' },
    },
    {
      path: '/search/text',
      name: 'search-text',
      component: () => import('@/views/TextSearchView.vue'),
      meta: { title: 'Text Search' },
    },
    {
      path: '/search/photo',
      name: 'search-photo',
      component: () => import('@/views/PhotoSearchView.vue'),
      meta: { title: 'Photo Search' },
    },
    {
      path: '/search/screen-record',
      name: 'search-screen-record',
      component: () => import('@/views/ScreenRecordView.vue'),
      meta: { title: 'Screen Record Search' },
    },
    {
      path: '/search/camera-live',
      name: 'search-camera-live',
      component: () => import('@/views/CameraLiveView.vue'),
      meta: { title: 'Camera Live Search' },
    },
    {
      path: '/src-windows/record-overlay',
      component: () => import('@/components/search/RecordOverlay.vue'),
    },
    {
      path: '/src-windows/answer-float',
      component: () => import('@/components/search/AnswerFloat.vue'),
    },
    {
      path: '/take/:code',
      name: 'take-exam',
      component: () => import('@/views/TakeExamView.vue'),
      meta: { title: 'Take Exam' },
    },
    {
      path: '/manage/:code',
      name: 'manage-exam',
      component: () => import('@/views/ManageResultsView.vue'),
      meta: { title: 'Exam Results' },
    },
    {
      path: '/mine',
      name: 'mine',
      component: () => import('@/views/MineView.vue'),
      meta: { title: 'Mine' },
    },
    {
      path: '/mine/config',
      name: 'mine-config',
      component: () => import('@/views/ConfigView.vue'),
      meta: { title: 'AI Config' },
    },
    {
      path: '/mine/settings',
      name: 'mine-settings',
      component: () => import('@/views/SettingsView.vue'),
      meta: { title: 'System Settings' },
    },
    {
      path: '/mine/published',
      name: 'mine-published',
      component: () => import('@/views/MyPublishedView.vue'),
      meta: { title: 'My Launched Exams' },
    },
    {
      path: '/mine/joined',
      name: 'mine-joined',
      component: () => import('@/views/MyJoinedView.vue'),
      meta: { title: 'My Joined Exams' },
    },
    {
      path: '/mine/joined/wrong',
      name: 'mine-joined-wrong',
      component: () => import('@/views/JoinedWrongView.vue'),
      meta: { title: 'Wrong Questions' },
    },
    {
      path: '/mine/records',
      name: 'mine-records',
      component: () => import('@/views/PracticeRecordsView.vue'),
      meta: { title: 'Practice Records' },
    },
    {
      path: '/config',
      redirect: '/mine/config',
    },
    {
      path: '/privacy',
      name: 'privacy',
      component: () => import('@/views/PrivacyView.vue'),
      meta: { title: 'Privacy Policy' },
    },
    {
      path: '/terms',
      name: 'terms',
      component: () => import('@/views/TermsView.vue'),
      meta: { title: 'Terms of Service' },
    },
    {
      path: '/auth/login',
      name: 'auth-login',
      component: () => import('@/views/platform/LoginView.vue'),
      meta: { title: 'Sign in', guestOnly: true },
    },
    {
      path: '/auth/register',
      name: 'auth-register',
      component: () => import('@/views/platform/RegisterView.vue'),
      meta: { title: 'Register', guestOnly: true },
    },
    {
      path: '/auth/pending',
      name: 'auth-pending',
      component: () => import('@/views/platform/PendingView.vue'),
      meta: { title: 'Account status', requiresAuth: true },
    },
    {
      path: '/platform',
      name: 'platform-home',
      component: () => import('@/views/platform/PlatformHomeView.vue'),
      meta: { title: 'Platform', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/admin',
      name: 'admin',
      component: () => import('@/views/AdminView.vue'),
      meta: { title: 'Admin' },
    },
  ],
})

// Platform accounts are optional: only routes that opt in via meta are guarded,
// so every existing (public) feature keeps working without an account.
router.beforeEach(async (to) => {
  const needsAuth = to.meta.requiresAuth || to.meta.guestOnly
  if (!needsAuth) return true
  if (!platformEnabled) return '/mine'

  const auth = useAuthStore()
  await auth.init()

  if (to.meta.guestOnly) return auth.isLoggedIn ? '/platform' : true
  if (!auth.isLoggedIn) return { path: '/auth/login', query: { redirect: to.fullPath } }
  if (to.meta.requiresActive && auth.profile && !auth.isActive) return '/auth/pending'
  const roles = to.meta.roles as string[] | undefined
  if (roles && (!auth.role || !roles.includes(auth.role))) return '/platform'
  return true
})

export default router
