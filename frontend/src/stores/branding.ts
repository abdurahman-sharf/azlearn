import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { platformEnabled, platformFetch } from '@/lib/platformApi'
import { brandCss } from '@/utils/brandTheme'

export interface PublicConfig {
  name: string | null
  color: string | null
  logo: string | null
  legal: { privacy: boolean; terms: boolean }
}

const CACHE_KEY = 'exameow-brand'
const STYLE_ID = 'platform-brand-style'
const EMPTY: PublicConfig = { name: null, color: null, logo: null, legal: { privacy: false, terms: false } }

function readCache(): PublicConfig {
  try {
    const raw = localStorage.getItem(CACHE_KEY)
    if (!raw) return EMPTY
    const c = JSON.parse(raw) as PublicConfig
    return { ...EMPTY, ...c, legal: { ...EMPTY.legal, ...(c.legal ?? {}) } }
  } catch {
    return EMPTY
  }
}

/** Replaces the injected <style>; text content only, and only ever our own generated CSS. */
function applyColor(color: string | null) {
  let el = document.getElementById(STYLE_ID) as HTMLStyleElement | null
  const css = color ? brandCss(color) : ''
  if (!css) {
    el?.remove()
    return
  }
  if (!el) {
    el = document.createElement('style')
    el.id = STYLE_ID
    document.head.appendChild(el)
  }
  el.textContent = css
}

export const useBrandingStore = defineStore('branding', () => {
  // The last known config is applied immediately (no colour flash), then refreshed from the server.
  const config = ref<PublicConfig>(platformEnabled ? readCache() : EMPTY)
  const loaded = ref(false)
  if (platformEnabled) applyColor(config.value.color)

  async function load() {
    if (!platformEnabled) return
    try {
      const fresh = await platformFetch<PublicConfig>('/public/config')
      config.value = fresh
      applyColor(fresh.color)
      title(fresh.name)
      try { localStorage.setItem(CACHE_KEY, JSON.stringify(fresh)) } catch { /* storage unavailable */ }
    } catch {
      /* offline or server error: keep the cached identity */
    } finally {
      loaded.value = true
    }
  }

  const name = computed(() => config.value.name)
  const title = (n: string | null) => {
    if (!platformEnabled) return
    document.title = n ?? 'azlearn'
    const icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (icon && n === null) icon.href = '/azlearn-favicon.png'
  }
  title(config.value.name)
  const logo = computed(() => config.value.logo)
  const legal = computed(() => config.value.legal)
  return { config, loaded, name, logo, legal, load }
})
