import { createRouter, createWebHashHistory } from 'vue-router'
import { setRecoveredHandler, useAuthStore } from '@/stores/auth'
import { platformEnabled, setUnauthorizedHandler } from '@/lib/platformApi'
import { pendingTeacherMayEnter } from '@/utils/platformGuard'
import { resetAdminStats } from '@/lib/adminStats'
import { resetTeacherStats } from '@/lib/teacherStats'
import { resetStudentStats } from '@/lib/studentStats'
import { setTitleKey } from '@/lib/docTitle'
import { focusStillOurs, shouldMoveFocus, titleKeyOf } from '@/utils/routeTitle'

/** Empty page for the old exam-editor addresses: the guard below always redirects away from them. */
const LEGACY_EDITOR = { render: () => null }

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      // Landing page on web/Docker (platform enabled); other builds keep opening on practice (see guard).
      path: '/',
      name: 'home',
      component: () => import('@/views/platform/LandingView.vue'),
      // landingHeader: the shell shows the landing menu (sections + sign in) instead of the practice/generate/search links
      meta: { title: 'Home', landingHeader: true },
    },
    {
      path: '/legal/:slug(privacy|terms)',
      name: 'legal',
      component: () => import('@/views/platform/LegalView.vue'),
      meta: { title: 'Legal' },
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
      meta: { titleKey: 'login', title: 'Sign in', guestOnly: true },
    },
    {
      path: '/auth/register',
      name: 'auth-register',
      component: () => import('@/views/platform/RegisterView.vue'),
      meta: { titleKey: 'register', title: 'Register', guestOnly: true },
    },
    {
      path: '/auth/pending',
      name: 'auth-pending',
      component: () => import('@/views/platform/PendingView.vue'),
      meta: { titleKey: 'ttlAccountStatus', title: 'Account status', requiresAuth: true },
    },
    {
      // Admin area (URLs unchanged). The sidebar is not part of this route: the app shell (AppShell) shows it on every
      // /platform page an admin opens, so shared pages (grading, notifications, account, …) keep it too.
      path: '/platform/admin',
      meta: { requiresAuth: true, requiresActive: true, roles: ['admin'] },
      children: [
        {
          path: '',
          name: 'platform-admin',
          component: () => import('@/views/platform/AdminOverviewView.vue'),
          meta: { titleKey: 'adminOverview', title: 'Overview', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'users',
          name: 'platform-admin-users',
          component: () => import('@/views/platform/AdminUsersView.vue'),
          meta: { titleKey: 'adminUsers', title: 'Accounts', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'institutions',
          name: 'platform-admin-institutions',
          component: () => import('@/views/platform/AdminInstitutionsView.vue'),
          meta: { titleKey: 'adminInstitutions', title: 'Institutions', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'institutions/:id',
          name: 'platform-admin-institution',
          component: () => import('@/views/platform/AdminInstitutionView.vue'),
          meta: { titleKey: 'ttlInstitution', title: 'Institution', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'teaching',
          name: 'platform-admin-teaching',
          component: () => import('@/views/platform/AdminTeachingView.vue'),
          meta: { titleKey: 'adminTeaching', title: 'platform-admin-teaching', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'reports',
          name: 'platform-admin-reports',
          component: () => import('@/views/platform/AdminReportsView.vue'),
          meta: { titleKey: 'adminReports', title: 'platform-admin-reports', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'exams',
          name: 'platform-admin-exams',
          component: () => import('@/views/platform/AdminExamsView.vue'),
          meta: { titleKey: 'exTitle', title: 'Exams', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'exams/new',
          name: 'platform-admin-exam-new',
          component: () => import('@/views/platform/AdminExamEditorView.vue'),
          meta: { titleKey: 'exNew', title: 'New exam', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'exams/:id/edit',
          name: 'platform-admin-exam-edit',
          component: () => import('@/views/platform/AdminExamEditorView.vue'),
          meta: { titleKey: 'ttlExamEdit', title: 'Edit exam', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'bank',
          name: 'platform-admin-bank',
          component: () => import('@/views/platform/AdminBankView.vue'),
          meta: { titleKey: 'bankTitle', title: 'Question bank', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'settings',
          name: 'platform-admin-settings',
          component: () => import('@/views/platform/AdminSettingsView.vue'),
          meta: { titleKey: 'settingsTitle', title: 'Settings', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'audit',
          name: 'platform-admin-audit',
          component: () => import('@/views/platform/AdminAuditView.vue'),
          meta: { titleKey: 'adminAudit', title: 'platform-admin-audit', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
        {
          path: 'system',
          name: 'platform-admin-system',
          component: () => import('@/views/platform/AdminSystemView.vue'),
          meta: { titleKey: 'sysTitle', title: 'System status', requiresAuth: true, requiresActive: true, roles: ['admin'] },
        },
      ],
    },
    {
      path: '/platform/grading',
      name: 'platform-grading',
      component: () => import('@/views/platform/GradingQueueView.vue'),
      meta: { titleKey: 'navGrading', title: 'Grading', requiresAuth: true, requiresActive: true, roles: ['teacher', 'admin'] },
    },
    {
      path: '/platform/grading/:id',
      name: 'platform-grade-exam',
      component: () => import('@/views/platform/GradeExamView.vue'),
      meta: { titleKey: 'navGrading', title: 'Grading', requiresAuth: true, requiresActive: true, roles: ['teacher', 'admin'] },
    },
    {
      // Teacher area home. Like the admin area, its sidebar is rendered by the app shell on every /platform page a teacher opens.
      path: '/platform/teacher',
      name: 'platform-teacher-home',
      component: () => import('@/views/platform/TeacherOverviewView.vue'),
      meta: { titleKey: 'tchOverview', title: 'Overview', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      // Student area home. Like the admin and teacher areas, its sidebar is rendered by the app shell on every /platform page a student opens.
      path: '/platform/student',
      name: 'platform-student-home',
      component: () => import('@/views/platform/StudentOverviewView.vue'),
      meta: { titleKey: 'stuOverview', title: 'Overview', requiresAuth: true, requiresActive: true, roles: ['student'] },
    },
    {
      // The student's exams (not /platform/exams: that is the teacher's list).
      path: '/platform/my-exams',
      name: 'platform-student-exams',
      component: () => import('@/views/platform/StudentExamsView.vue'),
      meta: { titleKey: 'stexTitle', title: 'My exams', requiresAuth: true, requiresActive: true, roles: ['student'] },
    },
    {
      path: '/platform/my-subjects',
      name: 'platform-student-subjects',
      component: () => import('@/views/platform/StudentSubjectsView.vue'),
      meta: { titleKey: 'mysubjTitle', title: 'My subjects', requiresAuth: true, requiresActive: true, roles: ['student'] },
    },
    {
      path: '/platform',
      name: 'platform-home',
      component: () => import('@/views/platform/PlatformHomeView.vue'),
      meta: { titleKey: 'platformHome', title: 'Platform', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/institutions/:id',
      name: 'platform-institution',
      component: () => import('@/views/platform/InstitutionBrowseView.vue'),
      meta: { titleKey: 'ttlInstitution', title: 'platform-institution', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/subjects/:id',
      name: 'platform-subject',
      component: () => import('@/views/platform/SubjectView.vue'),
      meta: { titleKey: 'ttlSubject', title: 'platform-subject', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/teachers',
      name: 'platform-teachers',
      component: () => import('@/views/platform/TeachersView.vue'),
      meta: { titleKey: 'browseTeachers', title: 'platform-teachers', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/teachers/:id',
      name: 'platform-teacher',
      component: () => import('@/views/platform/TeacherView.vue'),
      meta: { titleKey: 'ttlTeacher', title: 'platform-teacher', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/profile',
      name: 'platform-profile',
      component: () => import('@/views/platform/ProfileView.vue'),
      meta: { titleKey: 'myProfile', title: 'platform-profile', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/teaching',
      name: 'platform-teaching',
      component: () => import('@/views/platform/TeachingView.vue'),
      // pendingTeacherOk: a teacher whose account still awaits approval may already request subjects here
      // (the server allows exactly this page's four calls for such an account, see `may_request_teaching`).
      meta: { titleKey: 'tchNavSubjects', title: 'platform-teaching', requiresAuth: true, requiresActive: true, pendingTeacherOk: true, roles: ['teacher'] },
    },
    {
      path: '/platform/my-content',
      name: 'platform-my-content',
      component: () => import('@/views/platform/MyContentView.vue'),
      meta: { titleKey: 'myContent', title: 'platform-my-content', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/posts/new',
      name: 'platform-post-new',
      component: () => import('@/views/platform/PostEditorView.vue'),
      meta: { titleKey: 'newPost', title: 'platform-post-new', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/posts/:id/edit',
      name: 'platform-post-edit',
      component: () => import('@/views/platform/PostEditorView.vue'),
      meta: { titleKey: 'ttlPostEdit', title: 'platform-post-edit', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/posts/:id',
      name: 'platform-post',
      component: () => import('@/views/platform/PostView.vue'),
      meta: { titleKey: 'ttlPost', title: 'platform-post', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/courses/new',
      name: 'platform-course-new',
      component: () => import('@/views/platform/CourseEditorView.vue'),
      meta: { titleKey: 'newCourse', title: 'platform-course-new', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/courses/:id/edit',
      name: 'platform-course-edit',
      component: () => import('@/views/platform/CourseEditorView.vue'),
      meta: { titleKey: 'ttlCourseEdit', title: 'platform-course-edit', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/courses/:id',
      name: 'platform-course',
      component: () => import('@/views/platform/CourseView.vue'),
      meta: { titleKey: 'ttlCourse', title: 'platform-course', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/live/new',
      name: 'platform-live-new',
      component: () => import('@/views/platform/LiveEditorView.vue'),
      meta: { titleKey: 'newLive', title: 'platform-live-new', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/live/:id/edit',
      name: 'platform-live-edit',
      component: () => import('@/views/platform/LiveEditorView.vue'),
      meta: { titleKey: 'ttlLiveEdit', title: 'platform-live-edit', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/notifications',
      name: 'platform-notifications',
      component: () => import('@/views/platform/NotificationsView.vue'),
      meta: { titleKey: 'notifications', title: 'Notifications', requiresAuth: true, requiresActive: true },
    },
    {
      // My exams (the teacher's list) and the shared exam builder in teacher mode.
      path: '/platform/exams',
      name: 'platform-my-exams',
      component: () => import('@/views/platform/MyExamsView.vue'),
      meta: { titleKey: 'mexTitle', title: 'platform-my-exams', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/exams/new',
      name: 'platform-exam-new',
      component: () => import('@/views/platform/TeacherExamEditorView.vue'),
      meta: { titleKey: 'exNew', title: 'platform-exam-new', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      path: '/platform/exams/:id/edit',
      name: 'platform-exam-edit',
      component: () => import('@/views/platform/TeacherExamEditorView.vue'),
      meta: { titleKey: 'ttlExamEdit', title: 'platform-exam-edit', requiresAuth: true, requiresActive: true, roles: ['teacher'] },
    },
    {
      // The old exam editor is gone: its addresses lead to the new builder (an admin to the admin builder), keeping ?subject=.
      path: '/platform/assessments/new',
      name: 'platform-assessment-new',
      component: LEGACY_EDITOR,
      meta: { title: 'platform-assessment-new', requiresAuth: true, requiresActive: true, legacyEditor: 'new' },
    },
    {
      path: '/platform/assessments/:id/edit',
      name: 'platform-assessment-edit',
      component: LEGACY_EDITOR,
      meta: { title: 'platform-assessment-edit', requiresAuth: true, requiresActive: true, legacyEditor: 'edit' },
    },
    {
      path: '/platform/assessments/:id/take',
      name: 'platform-assessment-take',
      component: () => import('@/views/platform/TakeAssessmentView.vue'),
      meta: { titleKey: 'tkPageTitle', title: 'platform-assessment-take', requiresAuth: true, requiresActive: true, roles: ['student'] },
    },
    {
      path: '/platform/assessments/:id/results',
      name: 'platform-assessment-results',
      component: () => import('@/views/platform/AssessmentResultsView.vue'),
      meta: { titleKey: 'ttlExamResults', title: 'platform-assessment-results', requiresAuth: true, requiresActive: true, roles: ['teacher', 'admin'] },
    },
    {
      path: '/platform/assessments/:id',
      name: 'platform-assessment',
      component: () => import('@/views/platform/AssessmentView.vue'),
      meta: { titleKey: 'ttlExam', title: 'platform-assessment', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/attempts/:id',
      name: 'platform-attempt',
      component: () => import('@/views/platform/AttemptView.vue'),
      meta: { titleKey: 'attemptPageTitle', title: 'platform-attempt', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/search',
      name: 'platform-search',
      component: () => import('@/views/platform/SearchView.vue'),
      meta: { titleKey: 'tchNavSearch', title: 'platform-search', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/platform/account',
      name: 'platform-account',
      component: () => import('@/views/platform/AccountView.vue'),
      meta: { titleKey: 'accountSettings', title: 'platform-account', requiresAuth: true, requiresActive: true },
    },
    {
      path: '/admin',
      name: 'admin',
      component: () => import('@/views/AdminView.vue'),
      meta: { title: 'Admin' },
    },
    {
      // Anything else: a page that says so (with a way home) instead of an empty screen.
      path: '/:pathMatch(.*)*',
      name: 'not-found',
      component: () => import('@/views/NotFoundView.vue'),
      meta: { titleKey: 'nfTitle', title: 'Not found' },
    },
  ],
})

// Platform accounts are optional: only routes that opt in via meta are guarded,
// so every existing (public) feature keeps working without an account.
router.beforeEach(async (to) => {
  if (to.name === 'home') {
    if (!platformEnabled) return '/practice'
    const auth = useAuthStore()
    await auth.init()
    return auth.isLoggedIn ? '/platform' : true
  }
  if (to.name === 'legal' && !platformEnabled) return '/practice'
  const needsAuth = to.meta.requiresAuth || to.meta.guestOnly
  if (!needsAuth) return true
  if (!platformEnabled) return '/mine'

  const auth = useAuthStore()
  await auth.init()

  if (to.meta.guestOnly) return auth.isLoggedIn ? '/platform' : true
  // A session is stored but the profile could not be read (no network, server down): stay on the page instead of
  // sending the person to sign in. The shell shows an offline banner; once the profile arrives the guards run again
  // (see setRecoveredHandler below) and every role rule applies. The server enforces every rule meanwhile.
  if (!auth.isLoggedIn && auth.offline) return true
  if (!auth.isLoggedIn) return { path: '/auth/login', query: { redirect: to.fullPath, ...(auth.consumeSessionLost() ? { expired: '1' } : {}) } }
  // An approved account has nothing to wait for any more (a reload or a bookmark of the waiting page must not strand it).
  if (to.name === 'auth-pending' && auth.isActive) return '/platform'
  const pendingOk = pendingTeacherMayEnter(to.meta, auth.role, auth.profile?.status)
  if (to.meta.requiresActive && auth.profile && !auth.isActive && !pendingOk) return '/auth/pending'
  // The old exam-editor addresses lead to the new builder (an admin to the admin's), keeping the query (?subject=).
  const legacy = to.meta.legacyEditor as 'new' | 'edit' | undefined
  if (legacy) {
    const tail = legacy === 'new' ? 'new' : `${String(to.params.id)}/edit`
    return { path: `${auth.role === 'admin' ? '/platform/admin/exams' : '/platform/exams'}/${tail}`, query: to.query, replace: true }
  }
  // The student's home is their overview (admins and teachers are sent to theirs by the page itself).
  if (to.name === 'platform-home' && auth.role === 'student' && auth.isActive) return { path: '/platform/student', replace: true }
  const roles = to.meta.roles as string[] | undefined
  if (roles && (!auth.role || !roles.includes(auth.role))) return '/platform'
  return true
})

// ── After each navigation: the document title, and (after an in-app move to another page) focus on the new page's h1
// so a screen reader announces where the person is (WCAG 2.4.3). Only navigations the person caused move focus: a
// redirect while the app loads does not. The new heading may show up later (lazy page, data loading, out-in transition),
// so it is awaited for a few seconds; focus is never taken back from something the person focused meanwhile.
let firstNavigation = true
let lastInput = 0
if (typeof window !== 'undefined') {
  const mark = () => { lastInput = Date.now() }
  window.addEventListener('pointerdown', mark, true)
  window.addEventListener('keydown', mark, true)
  window.addEventListener('popstate', mark) // the browser's back/forward buttons
}

function focusHeadingSoon() {
  const main = document.getElementById('main-content')
  if (!main) return
  const old = main.querySelector('h1')
  const atNavigation = document.activeElement
  let settled = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let observer: MutationObserver | null = null
  const stop = () => {
    settled = true
    observer?.disconnect()
    if (timer) clearTimeout(timer)
  }
  const attempt = (): boolean => {
    if (settled) return true
    const h = [...main.querySelectorAll<HTMLElement>('h1')].find((x) => x !== old && x.isConnected)
    if (!h) return false
    stop()
    if (!focusStillOurs(document.activeElement, atNavigation, document.body, main)) return true
    if (!h.hasAttribute('tabindex')) h.setAttribute('tabindex', '-1')
    h.focus({ preventScroll: true })
    return true
  }
  if (attempt()) return
  observer = new MutationObserver(() => { attempt() })
  observer.observe(main, { childList: true, subtree: true })
  timer = setTimeout(stop, 5000)
}

router.afterEach((to, from, failure) => {
  if (failure) return
  setTitleKey(titleKeyOf(to.meta))
  const first = firstNavigation
  firstNavigation = false
  if (!platformEnabled || typeof document === 'undefined') return
  if (shouldMoveFocus(to, from, first) && Date.now() - lastInput < 10_000) focusHeadingSoon()
})

// A request that carried a session the server no longer knows (expired, signed out elsewhere, suspended mid-session):
// forget the local session and send the user to sign in again, coming back to the page they were on.
setUnauthorizedHandler(() => {
  useAuthStore().clearSession()
  resetAdminStats()
  resetTeacherStats()
  resetStudentStats()
  const here = router.currentRoute.value
  // Public pages (and the very first navigation, whose own guard redirects) stay where they are.
  // `expired` makes the sign-in page say why the person is there (and, on an exam page, that the answers are kept).
  if (here.meta.requiresAuth) router.replace({ path: '/auth/login', query: { redirect: here.fullPath, expired: '1' } })
})

// The profile arrived after the network failed at start-up: run the guards again for the page the person is on (a role
// page they may not open, a guest page they no longer need, the student home...). `force` because it is the same address.
setRecoveredHandler(() => {
  const here = router.currentRoute.value
  router.replace({ path: here.path, query: here.query, hash: here.hash, force: true }).catch(() => { /* a redirect is not an error */ })
})

export default router
