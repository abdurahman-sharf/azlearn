<script setup lang="ts">
import { computed, ref, toRef, type Component } from 'vue'
import { useCountUp } from '@/composables/useCountUp'

// One landing-page counter: counts up when it scrolls into view. The animated digits are hidden from assistive
// technology; a visually hidden copy carries the final value.
const props = defineProps<{ value: number; label: string; icon: Component }>()
const el = ref<HTMLElement | null>(null)
const shown = useCountUp(toRef(props, 'value'), el)
const fmt = new Intl.NumberFormat('en')
const final = computed(() => fmt.format(props.value))
</script>

<template>
  <li ref="el" class="flex flex-col items-center gap-1 text-center px-2" data-testid="stat-counter">
    <component :is="props.icon" class="w-7 h-7 mb-1" style="color: rgb(var(--azl-teal))" aria-hidden="true" />
    <span class="text-display-sm font-extrabold tracking-tight" style="color: #fff">
      <span dir="ltr" class="inline-block" aria-hidden="true" data-testid="stat-animated">{{ fmt.format(shown) }}</span>
      <span class="sr-only" data-testid="stat-final">{{ final }}</span>
    </span>
    <span class="text-body-md font-semibold" style="color: rgb(219 234 254)">{{ props.label }}</span>
  </li>
</template>
