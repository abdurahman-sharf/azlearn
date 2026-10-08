<script setup lang="ts">
import { computed, nextTick, ref, watch, type Component } from 'vue'
import { useRoute } from 'vue-router'
import { ArrowRightOnRectangleIcon } from '@heroicons/vue/24/outline'
import { useAuthStore } from '@/stores/auth'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { useSignOut } from '@/composables/useSignOut'

export interface NavItem { to: string; label: PlatformKey; icon: Component; badge?: number; exact?: boolean; testid: string }
export interface NavGroup {
  key: string
  /** heading of the group (desktop); null = no heading */
  label: PlatformKey | null
  /** draw a thin rule above the group (desktop), for a heading-less group that starts a new block */
  separator?: boolean
  items: NavItem[]
}

// The navigation shared by every role's area. It is rendered by the app shell next to <main> on every /platform page
// (one nav in the DOM: a column on desktop, a sticky scrolling bar under the header on small screens). A role only
// supplies its groups and a few labels/test ids; the look, the badges, the user chip and the sign-out are common.
const props = defineProps<{
  groups: NavGroup[]
  /** names the CSS classes (`<variant>-nav`, `<variant>-nav-link`) */
  variant: string
  /** accessible name of the <nav> landmark */
  navLabel: PlatformKey
  /** the role shown under the user's name in the chip */
  roleLabel: PlatformKey
  /** data-testid of the <nav> */
  testid: string
  /** data-testid of the user chip */
  userTestid: string
  /** prefix of the ids that label each group's list */
  groupIdPrefix: string
}>()

const pt = usePt()
const auth = useAuthStore()
const route = useRoute()
const signOut = useSignOut()
const navEl = ref<HTMLElement | null>(null)

// keep the current section visible in the mobile bar
watch(() => route.path, () => {
  nextTick(() => navEl.value?.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ inline: 'center', block: 'nearest' }))
})

const isActive = (i: { to: string; exact?: boolean }) => (i.exact ? route.path === i.to : route.path === i.to || route.path.startsWith(i.to + '/'))
const initial = computed(() => (auth.profile?.full_name || auth.profile?.email || '?').trim().charAt(0).toUpperCase())
</script>

<template>
  <nav
    ref="navEl"
    :aria-label="pt(props.navLabel)"
    :class="props.variant + '-nav'"
    class="w-full md:w-64 shrink-0 sticky z-20 top-14 sm:top-16 md:top-20 md:self-start md:max-h-[calc(100vh-6rem)] md:overflow-y-auto -mx-3 sm:-mx-6 md:mx-0 px-3 sm:px-6 md:px-0 py-2 md:py-0 border-b md:border-0"
    style="background-color: rgb(var(--md-surface)); border-color: rgb(var(--md-outline-variant))"
    :data-testid="props.testid"
  >
    <!-- who is signed in (desktop) -->
    <div class="hidden md:flex items-center gap-3 rounded-2xl p-3 mb-3 card-filled" :data-testid="props.userTestid">
      <span class="w-10 h-10 shrink-0 rounded-full flex items-center justify-center font-bold" style="background-color: rgb(var(--md-primary)); color: rgb(var(--md-on-primary))" aria-hidden="true">{{ initial }}</span>
      <span class="min-w-0">
        <span class="block font-bold truncate">{{ auth.profile?.full_name }}</span>
        <span class="block text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt(props.roleLabel) }}</span>
      </span>
    </div>

    <div class="flex md:flex-col gap-1 overflow-x-auto md:overflow-visible snap-x md:snap-none pb-1 md:pb-0">
      <template v-for="g in props.groups" :key="g.key">
        <span v-if="g.label" :id="props.groupIdPrefix + g.key" class="hidden md:block px-3 pt-3 pb-1 text-label-lg font-bold" style="color: rgb(var(--md-on-surface-variant))">{{ pt(g.label) }}</span>
        <span v-else-if="g.separator" class="hidden md:block my-2 border-t" style="border-color: rgb(var(--md-outline-variant))" aria-hidden="true"></span>
        <ul :aria-labelledby="g.label ? props.groupIdPrefix + g.key : undefined" class="flex md:flex-col gap-1">
          <li v-for="i in g.items" :key="i.to" class="shrink-0 snap-start">
            <router-link
              :to="i.to"
              :data-testid="i.testid"
              :aria-current="isActive(i) ? 'page' : undefined"
              class="flex items-center gap-3 whitespace-nowrap rounded-xl border-s-4 px-3 py-2.5 text-body-md font-semibold no-underline transition-colors"
              :class="isActive(i) ? '' : props.variant + '-nav-link'"
              :style="isActive(i)
                ? { backgroundColor: 'rgb(var(--md-primary))', color: 'rgb(var(--md-on-primary))', borderColor: 'rgb(var(--azl-deep-blue))' }
                : { color: 'rgb(var(--md-on-surface))', borderColor: 'transparent' }"
            >
              <component :is="i.icon" class="w-5 h-5 shrink-0" aria-hidden="true" />
              <span class="flex-1">{{ pt(i.label) }}</span>
              <span
                v-if="i.badge"
                class="min-w-[1.5rem] text-center rounded-full px-2 text-xs font-bold"
                :data-testid="i.testid + '-badge'"
                style="background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))"
              ><span dir="ltr" class="inline-block">{{ i.badge }}</span></span>
            </router-link>
          </li>
        </ul>
      </template>
      <button
        type="button"
        class="shrink-0 flex items-center gap-3 whitespace-nowrap rounded-xl border-s-4 border-transparent px-3 py-2.5 text-body-md font-semibold text-start"
        :class="props.variant + '-nav-link'"
        style="color: rgb(var(--md-on-surface))"
        data-testid="nav-logout"
        @click="signOut"
      >
        <ArrowRightOnRectangleIcon class="w-5 h-5 shrink-0 rtl:-scale-x-100" aria-hidden="true" />
        <span>{{ pt('logout') }}</span>
      </button>
    </div>
  </nav>
</template>

<style scoped>
.admin-nav-link:hover,
.teacher-nav-link:hover { background-color: rgb(var(--md-primary-container)); }
</style>
