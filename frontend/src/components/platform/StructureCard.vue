<script setup lang="ts">
import type { Component } from 'vue'
import type { RouteLocationRaw } from 'vue-router'
import { EyeIcon, EyeSlashIcon, PencilSquareIcon, TrashIcon } from '@heroicons/vue/24/outline'
import { usePt } from '@/i18n/platform'

// A card of the study structure (institution, department/level/year/term, subject): name, kind badge, a few numbers,
// and the edit / hide / delete actions. The whole card opens `to` (stretched link on the title); the action buttons sit
// above it. Hidden items stay visible to the admin with a badge and a dashed border (never faded: faded text fails contrast).
export interface CardStat { label: string; value: number }
const props = defineProps<{
  title: string
  to?: RouteLocationRaw
  subtitle?: string
  badge?: string
  icon?: Component
  inactive?: boolean
  stats?: CardStat[]
  /** names of what the card contains (the next level), shown as small chips */
  chips?: string[]
  moreChips?: number
  /** heading level of the title: 2 when the page has no section heading above the cards, else 3 */
  level?: 2 | 3
  testid?: string
}>()
const emit = defineEmits<{ edit: []; toggle: []; remove: [] }>()
const pt = usePt()
</script>

<template>
  <article
    class="card-filled relative flex flex-col gap-3 p-4 transition-shadow hover:shadow-md has-[a:focus-visible]:ring-2"
    :class="props.inactive ? '!border-dashed' : ''"
    style="--tw-ring-color: rgb(var(--md-primary))"
    :data-testid="props.testid ?? 'structure-card'"
  >
    <div class="flex items-start gap-3">
      <span v-if="props.icon" class="w-11 h-11 shrink-0 rounded-2xl flex items-center justify-center" style="background-color: rgb(var(--md-primary-container)); color: rgb(var(--md-on-primary-container))" aria-hidden="true">
        <component :is="props.icon" class="w-6 h-6" />
      </span>
      <div class="min-w-0 flex-1">
        <component :is="props.level === 2 ? 'h2' : 'h3'" class="font-bold text-title-md leading-snug break-words">
          <router-link v-if="props.to" :to="props.to" class="no-underline after:absolute after:inset-0 after:content-['']" data-testid="card-open">{{ props.title }}</router-link>
          <template v-else>{{ props.title }}</template>
        </component>
        <p v-if="props.subtitle" class="text-body-sm break-words" style="color: rgb(var(--md-on-surface-variant))">{{ props.subtitle }}</p>
        <div class="flex flex-wrap gap-1.5 mt-1.5">
          <span v-if="props.badge" class="px-2 py-0.5 rounded-full text-xs font-semibold" style="background-color: rgb(var(--md-secondary-container)); color: rgb(var(--md-on-secondary-container))">{{ props.badge }}</span>
          <span v-if="props.inactive" class="px-2 py-0.5 rounded-full text-xs font-semibold border border-dashed" style="color: rgb(var(--md-on-surface)); border-color: rgb(var(--md-on-surface-variant))" data-testid="card-hidden">{{ pt('inactive') }}</span>
        </div>
      </div>
    </div>

    <ul v-if="props.stats?.length" class="grid gap-2" :style="{ gridTemplateColumns: `repeat(${props.stats.length}, minmax(0, 1fr))` }" data-testid="card-stats">
      <li v-for="s in props.stats" :key="s.label" class="rounded-xl px-2 py-2 text-center" style="background-color: rgb(var(--md-surface))">
        <span class="block font-bold text-title-md"><span dir="ltr" class="inline-block">{{ s.value }}</span></span>
        <span class="block text-xs leading-tight" style="color: rgb(var(--md-on-surface-variant))">{{ s.label }}</span>
      </li>
    </ul>

    <ul v-if="props.chips?.length" class="flex flex-wrap gap-1.5" :aria-label="pt('stcContains')">
      <li v-for="c in props.chips" :key="c" class="px-2.5 py-0.5 rounded-full text-sm" style="background-color: rgb(var(--md-surface)); color: rgb(var(--md-on-surface))">{{ c }}</li>
      <li v-if="props.moreChips" class="px-2.5 py-0.5 rounded-full text-sm font-semibold" style="color: rgb(var(--md-on-surface-variant))"><span dir="ltr" class="inline-block">+{{ props.moreChips }}</span></li>
    </ul>

    <div class="relative z-10 flex items-center gap-1 mt-auto pt-1 border-t" style="border-color: rgb(var(--md-outline-variant))">
      <button type="button" class="card-act" :aria-label="`${pt('edit')}: ${props.title}`" :title="pt('edit')" data-testid="card-edit" @click="emit('edit')">
        <PencilSquareIcon class="w-5 h-5" aria-hidden="true" />
      </button>
      <button type="button" class="card-act" :aria-label="`${props.inactive ? pt('stcShow') : pt('stcHide')}: ${props.title}`" :title="props.inactive ? pt('stcShow') : pt('stcHide')" data-testid="card-toggle" @click="emit('toggle')">
        <EyeIcon v-if="props.inactive" class="w-5 h-5" aria-hidden="true" />
        <EyeSlashIcon v-else class="w-5 h-5" aria-hidden="true" />
      </button>
      <button type="button" class="card-act" :aria-label="`${pt('del')}: ${props.title}`" :title="pt('del')" style="color: rgb(var(--md-error))" data-testid="card-delete" @click="emit('remove')">
        <TrashIcon class="w-5 h-5" aria-hidden="true" />
      </button>
    </div>
  </article>
</template>

<style scoped>
.card-act {
  width: 2.5rem;
  height: 2.5rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 9999px;
  color: rgb(var(--md-on-surface));
}
.card-act:hover { background-color: rgb(var(--md-primary-container)); }
</style>
