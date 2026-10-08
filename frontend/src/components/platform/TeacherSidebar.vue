<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import {
  ArchiveBoxIcon, BellIcon, BookOpenIcon, ClipboardDocumentCheckIcon, Cog6ToothIcon, GlobeAltIcon, HomeIcon,
  MagnifyingGlassIcon, RectangleStackIcon, UserCircleIcon, UsersIcon,
} from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { useTeacherStats } from '@/lib/teacherStats'
import RoleSidebar, { type NavGroup } from './RoleSidebar.vue'

// The teacher area's navigation (the shared RoleSidebar with the teacher's groups). The legacy practice/generate/search
// tabs are hidden for teachers inside /platform, so the one link to the local question bank (/generate) lives here.
const auth = useAuthStore()
const route = useRoute()
const { stats, refresh } = useTeacherStats()

onMounted(refresh)
// Grading a batch or reading notifications changes the badges; refresh when moving between sections.
watch(() => route.path, refresh)

const groups = computed<NavGroup[]>(() => [
  { key: 'home', label: null, items: [
    { to: '/platform/teacher', label: 'tchOverview', icon: HomeIcon, exact: true, testid: 'nav-teacher-overview' },
  ] },
  { key: 'teaching', label: 'tchGroupTeaching', items: [
    { to: '/platform/teaching', label: 'tchNavSubjects', icon: BookOpenIcon, testid: 'nav-teacher-teaching' },
    { to: '/platform/my-content', label: 'myContent', icon: RectangleStackIcon, testid: 'nav-teacher-content' },
    { to: '/platform/grading', label: 'navGrading', icon: ClipboardDocumentCheckIcon, badge: stats.value?.pending_grading?.answers, testid: 'nav-teacher-grading' },
  ] },
  { key: 'comms', label: 'tchGroupComms', items: [
    { to: '/platform/notifications', label: 'notifications', icon: BellIcon, badge: stats.value?.unread, testid: 'nav-teacher-notifications' },
    // the way to subjects, courses and colleagues now that the old home (with its search box and teacher list) is gone
    { to: '/platform/search', label: 'tchNavSearch', icon: MagnifyingGlassIcon, testid: 'nav-teacher-search' },
    { to: '/platform/teachers', label: 'browseTeachers', icon: UsersIcon, exact: true, testid: 'nav-teacher-directory' },
  ] },
  { key: 'me', label: 'tchGroupMe', items: [
    { to: '/platform/profile', label: 'myProfile', icon: UserCircleIcon, testid: 'nav-teacher-profile' },
    ...(auth.profile ? [{ to: `/platform/teachers/${auth.profile.id}`, label: 'tchPublicPage' as const, icon: GlobeAltIcon, testid: 'nav-teacher-public' }] : []),
    { to: '/platform/account', label: 'accountSettings', icon: Cog6ToothIcon, testid: 'nav-teacher-account' },
    { to: '/generate', label: 'tchLocalBank', icon: ArchiveBoxIcon, testid: 'nav-teacher-local-bank' },
  ] },
])
</script>

<template>
  <RoleSidebar
    :groups="groups"
    variant="teacher"
    nav-label="teacherNav"
    role-label="roleTeacher"
    testid="teacher-nav"
    user-testid="teacher-user"
    group-id-prefix="tch-g-"
  />
</template>
