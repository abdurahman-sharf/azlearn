import { nextTick, onBeforeUnmount, watch, type Ref } from 'vue'

const FOCUSABLE = 'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])'

/**
 * Keyboard behaviour every modal dialog needs (WCAG 2.1.2 / 2.4.3): focus moves into it when it opens (the element
 * marked `data-autofocus`, else the first focusable one), Tab/Shift+Tab cycle inside it, Escape closes it, and focus
 * returns to whatever opened it. Works for dialogs toggled by a flag (`open`) and for ones that exist only while
 * mounted (pass `ref(true)`).
 */
export function useDialog(open: Ref<boolean>, container: Ref<HTMLElement | null>, close: () => void) {
  let opener: HTMLElement | null = null
  let listening = false

  function items(): HTMLElement[] {
    return [...(container.value?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter((el) => el.offsetParent !== null || el === document.activeElement)
  }

  function onKey(e: KeyboardEvent) {
    if (!open.value || !container.value) return
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
      return
    }
    if (e.key !== 'Tab') return
    const list = items()
    if (!list.length) {
      e.preventDefault()
      return
    }
    const first = list[0]!
    const last = list[list.length - 1]!
    const active = document.activeElement as HTMLElement | null
    const inside = !!active && container.value.contains(active)
    if (e.shiftKey && (!inside || active === first)) {
      e.preventDefault()
      last.focus()
    } else if (!e.shiftKey && (!inside || active === last)) {
      e.preventDefault()
      first.focus()
    }
  }

  function listen(on: boolean) {
    if (on && !listening) document.addEventListener('keydown', onKey, true)
    if (!on && listening) document.removeEventListener('keydown', onKey, true)
    listening = on
  }

  function restore() {
    listen(false)
    const o = opener
    opener = null
    if (o && document.contains(o)) o.focus()
  }

  watch(
    open,
    async (v) => {
      if (v) {
        opener = document.activeElement as HTMLElement | null
        listen(true)
        await nextTick()
        const el = container.value?.querySelector<HTMLElement>('[data-autofocus]') ?? items()[0]
        el?.focus()
      } else {
        restore()
      }
    },
    { immediate: true, flush: 'post' },
  )

  onBeforeUnmount(restore)
}
