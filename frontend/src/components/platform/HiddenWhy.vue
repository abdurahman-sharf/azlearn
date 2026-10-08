<script setup lang="ts">
import { computed } from 'vue'
import { usePt, type PlatformKey } from '@/i18n/platform'
import type { HiddenReason } from '@/api/platformContent'
import { hiddenReasonLink } from '@/utils/contentHub'

// The sentence under a "hidden" chip: why students cannot see the item and what the owner can do about it. Only a missing
// assignment is the owner's to fix (a link to My subjects); a switched-off subject, institution or account is explained.
const props = defineProps<{ reason: HiddenReason | null | undefined }>()
const pt = usePt()
const link = computed(() => hiddenReasonLink(props.reason))
</script>

<template>
  <p v-if="reason" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))" data-testid="content-hidden-why">
    {{ pt(`hubWhy_${reason}` as PlatformKey) }}
    <router-link v-if="link" :to="link" class="underline font-semibold" data-testid="content-hidden-link">{{ pt('hubWhyLink') }}</router-link>
  </p>
</template>
