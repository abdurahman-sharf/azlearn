// Pure rules of the document title and of the focus move after a navigation (kept out of the router so a plain script
// can test them).

/** "<page> · <brand>", or the brand alone when the page has no title of its own. */
export function composeTitle(page: string | null | undefined, brand: string): string {
  const p = (page ?? '').trim()
  const b = brand.trim()
  if (!p) return b
  if (!b || p === b) return p
  return `${p} · ${b}`
}

/** The translation key a route declares in `meta.titleKey` (null when it declares none). */
export function titleKeyOf(meta: Record<PropertyKey, unknown> | null | undefined): string | null {
  const k = meta?.titleKey
  return typeof k === 'string' && k ? k : null
}

/**
 * Whether focus should move to the new page's heading: after an in-app navigation to another page only - never on the
 * first load (the browser starts at the top anyway) and never when only the query or hash changed (a filter, a tab).
 */
export function shouldMoveFocus(to: { path: string }, from: { path: string }, firstNavigation: boolean): boolean {
  return !firstNavigation && to.path !== from.path
}

/**
 * Whether it is still fine to move focus to the heading once it shows up: the person has not put focus somewhere else in
 * the meantime. `active` is the focused element now, `atNavigation` the one focused when the navigation happened.
 */
export function focusStillOurs(
  active: { isConnected: boolean } | null,
  atNavigation: { isConnected: boolean } | null,
  body: unknown,
  main: unknown,
): boolean {
  return !active || active === body || active === main || active === atNavigation || !active.isConnected
}
