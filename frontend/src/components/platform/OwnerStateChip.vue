<script setup lang="ts">
import { computed } from 'vue'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { ownerState, type OwnerRow, type OwnerState } from '@/utils/contentHub'

// The one chip that says how an owner's own item stands: draft, published, scheduled, ended, cancelled, closed,
// archived - or "hidden: <reason>" when it is published but students cannot see it (the green "published" would lie).
const props = defineProps<{ row: OwnerRow }>()
const pt = usePt()

const state = computed<OwnerState>(() => ownerState(props.row))
const LABEL: Record<Exclude<OwnerState, 'hidden'>, PlatformKey> = {
  draft: 'statusDraft', published: 'statusPublished', scheduled: 'hubScheduled', cancelled: 'cancelled',
  ended: 'hubEnded', closed: 'exPhase_closed', archived: 'exPhase_archived',
}
const label = computed(() => {
  if (state.value !== 'hidden') return pt(LABEL[state.value])
  const reason = props.row.hidden_reason
  return reason ? `${pt('hubHidden')}: ${pt(`hubReason_${reason}` as PlatformKey)}` : pt('hubHidden')
})
</script>

<template>
  <span
    class="text-xs font-semibold px-2 py-0.5 rounded-full shrink-0 break-words"
    :style="state === 'hidden' ? 'background-color: rgb(var(--azl-amber)); color: rgb(var(--azl-on-amber))' : 'background-color: rgb(var(--md-surface-container-high))'"
    :data-testid="state === 'hidden' ? 'content-hidden-chip' : `content-state-${state}`"
  >{{ label }}</span>
</template>
