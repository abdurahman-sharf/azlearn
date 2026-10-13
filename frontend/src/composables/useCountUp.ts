import { onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'

/**
 * A number that counts up to `target` once its element scrolls into view. No animation (the final value at once) when
 * the user asks for reduced motion or the browser has no IntersectionObserver.
 */
export function useCountUp(target: Ref<number>, el: Ref<HTMLElement | null>, durationMs = 1400) {
  const shown = ref(0)
  let frame = 0
  let visible = false
  let observer: IntersectionObserver | null = null

  const reduced = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches

  function run() {
    cancelAnimationFrame(frame)
    const to = target.value
    if (reduced() || !visible) {
      shown.value = visible ? to : shown.value
      return
    }
    const from = shown.value
    const start = performance.now()
    const tick = (now: number) => {
      const p = Math.min(1, (now - start) / durationMs)
      shown.value = Math.round(from + (to - from) * (1 - Math.pow(1 - p, 3)))
      if (p < 1) frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
  }

  onMounted(() => {
    if (typeof IntersectionObserver === 'undefined' || !el.value) {
      visible = true
      shown.value = target.value
      return
    }
    observer = new IntersectionObserver((entries) => {
      if (!entries.some((e) => e.isIntersecting)) return
      visible = true
      observer?.disconnect()
      run()
    })
    observer.observe(el.value)
  })
  watch(target, () => { if (visible) run() })
  onBeforeUnmount(() => {
    cancelAnimationFrame(frame)
    observer?.disconnect()
  })
  return shown
}
