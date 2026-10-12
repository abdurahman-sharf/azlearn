import { platformEnabled } from '@/lib/platformApi'
import { usePt, type PlatformKey } from '@/i18n/platform'
import { composeTitle } from '@/utils/routeTitle'

// The document title: "<page> · <brand>". The router reports the page (its `meta.titleKey`), the branding store the
// platform's name, and the app shell asks for a refresh when the language changes. Kept apart from the branding store so
// neither imports the other. Builds without the platform keep their static <title>.
let titleKey: string | null = null
let brandName: string | null = null

function apply() {
  if (!platformEnabled || typeof document === 'undefined') return
  try {
    const pt = usePt()
    const page = titleKey ? (pt(titleKey as PlatformKey) as string | undefined) : null
    document.title = composeTitle(page, brandName ?? pt('defaultPlatformName'))
  } catch {
    /* no active store yet (very first module evaluation): the next call sets it */
  }
}

/** The current page's title key (from `meta.titleKey`; null = the brand alone). */
export function setTitleKey(key: string | null) {
  titleKey = key
  apply()
}

/** The platform's own name (null = the default name). */
export function setTitleBrand(name: string | null) {
  brandName = name
  apply()
}

/** Re-renders the title (e.g. after a language change). */
export const refreshDocumentTitle = apply
