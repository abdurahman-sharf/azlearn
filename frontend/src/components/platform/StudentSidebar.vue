<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import {
  AcademicCapIcon, BellIcon, BookOpenIcon, BookmarkIcon, CalendarDaysIcon, ClipboardDocumentListIcon, Cog6ToothIcon, HomeIcon, MagnifyingGlassIcon,
  PlayCircleIcon, UserCircleIcon, UsersIcon,
} from '@heroicons/vue/24/outline'
import { useStudentStats } from '@/lib/studentStats'
import { examsBadge } from '@/utils/studentExams'
import RoleSidebar, { type NavGroup } from './RoleSidebar.vue'

// The student area's navigation (the shared RoleSidebar with the student's groups). The legacy practice/generate/search
// tabs are hidden for students inside /platform, so the one link to the local practice tools (/practice) lives here.
const route = useRoute()
const { stats, refresh } = useStudentStats()

onMounted(refresh)
// Starting an exam or reading notifications changes the badges; refresh when moving between sections.
watch(() => route.path, refresh)

const groups = computed<NavGroup[]>(() => [
  { key: 'home', label: null, items: [
    { to: '/platform/student', label: 'stuOverview', icon: HomeIcon, exact: true, testid: 'nav-student-home' },
    { to: '/platform/notifications', label: 'notifications', icon: BellIcon, badge: stats.value?.unread, testid: 'nav-student-notifications' },
  ] },
  { key: 'study', label: 'stuGroupStudy', items: [
    { to: '/platform/my-subjects', label: 'mysubjTitle', icon: BookOpenIcon, testid: 'nav-student-subjects' },
    { to: '/platform/learning', label: 'lrnTitle', icon: PlayCircleIcon, testid: 'nav-student-learning' },
    { to: '/platform/my-exams', label: 'stexTitle', icon: ClipboardDocumentListIcon, badge: examsBadge(stats.value), testid: 'nav-student-exams' },
    { to: '/platform/schedule', label: 'schTitle', icon: CalendarDaysIcon, testid: 'nav-student-schedule' },
  ] },
  { key: 'explore', label: 'stuGroupExplore', items: [
    { to: '/platform/teachers', label: 'browseTeachers', icon: UsersIcon, testid: 'nav-student-teachers' },
    { to: '/platform/following', label: 'fllTitle', icon: BookmarkIcon, testid: 'nav-student-following' },
    { to: '/platform/search', label: 'tchNavSearch', icon: MagnifyingGlassIcon, testid: 'nav-student-search' },
  ] },
  { key: 'me', label: 'tchGroupMe', items: [
    { to: '/platform/profile', label: 'myProfile', icon: UserCircleIcon, testid: 'nav-student-profile' },
    { to: '/platform/account', label: 'accountSettings', icon: Cog6ToothIcon, testid: 'nav-student-account' },
    { to: '/practice', label: 'stuPracticeTools', icon: AcademicCapIcon, testid: 'nav-student-practice' },
  ] },
])
</script>

<template>
  <RoleSidebar
    :groups="groups"
    variant="student"
    nav-label="stuNav"
    role-label="roleStudent"
    testid="student-nav"
    user-testid="student-user"
    group-id-prefix="stu-g-"
  />
</template>
