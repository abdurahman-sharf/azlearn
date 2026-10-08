<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import {
  AcademicCapIcon, ArchiveBoxIcon, BellIcon, BuildingLibraryIcon, ClipboardDocumentCheckIcon,
  ClipboardDocumentListIcon, Cog6ToothIcon, DocumentMagnifyingGlassIcon, FlagIcon, HomeIcon, ServerStackIcon, UserCircleIcon, UsersIcon,
} from '@heroicons/vue/24/outline'
import { useAdminStats } from '@/lib/adminStats'
import RoleSidebar, { type NavGroup } from './RoleSidebar.vue'

// The admin area's navigation: the groups below inside the shared RoleSidebar (markup, test ids and behaviour are unchanged).
const route = useRoute()
const { stats, refresh } = useAdminStats()

onMounted(refresh)
// Approving/rejecting something changes the badges; refresh when moving between sections.
watch(() => route.path, refresh)

const groups = computed<NavGroup[]>(() => [
  { key: 'home', label: null, items: [
    { to: '/platform/admin', label: 'adminOverview', icon: HomeIcon, exact: true, testid: 'nav-overview' },
  ] },
  { key: 'learning', label: 'admGroupLearning', items: [
    { to: '/platform/admin/exams', label: 'exTitle', icon: ClipboardDocumentListIcon, testid: 'nav-exams' },
    { to: '/platform/grading', label: 'navGrading', icon: ClipboardDocumentCheckIcon, badge: stats.value?.pending_grading, testid: 'nav-grading' },
    { to: '/platform/admin/bank', label: 'bankTitle', icon: ArchiveBoxIcon, testid: 'nav-bank' },
    { to: '/platform/admin/institutions', label: 'adminInstitutions', icon: BuildingLibraryIcon, testid: 'nav-institutions' },
  ] },
  { key: 'people', label: 'admGroupPeople', items: [
    { to: '/platform/admin/users', label: 'adminUsers', icon: UsersIcon, badge: stats.value?.pending_teachers, testid: 'nav-users' },
    { to: '/platform/admin/teaching', label: 'adminTeaching', icon: AcademicCapIcon, badge: stats.value?.pending_teaching, testid: 'nav-teaching' },
    { to: '/platform/admin/reports', label: 'adminReports', icon: FlagIcon, badge: stats.value?.open_reports, testid: 'nav-reports' },
  ] },
  { key: 'system', label: 'admGroupSystem', items: [
    { to: '/platform/admin/audit', label: 'adminAudit', icon: DocumentMagnifyingGlassIcon, testid: 'nav-audit' },
    { to: '/platform/admin/settings', label: 'settingsTitle', icon: Cog6ToothIcon, testid: 'nav-settings' },
    { to: '/platform/admin/system', label: 'sysTitle', icon: ServerStackIcon, badge: stats.value?.system_warnings, testid: 'nav-system' },
  ] },
  { key: 'me', label: null, separator: true, items: [
    { to: '/platform/notifications', label: 'notifications', icon: BellIcon, testid: 'nav-notifications' },
    { to: '/platform/account', label: 'accountSettings', icon: UserCircleIcon, testid: 'nav-account' },
  ] },
])
</script>

<template>
  <RoleSidebar
    :groups="groups"
    variant="admin"
    nav-label="adminNav"
    role-label="roleAdmin"
    testid="admin-nav"
    user-testid="admin-user"
    group-id-prefix="adm-g-"
  />
</template>
