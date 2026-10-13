/**
 * An in-app path from the server (`/platform/...`, optionally with a plain query), or null for anything else. The server
 * builds these links itself, but they end up in `href`s and router pushes, so an absolute URL, a protocol-relative one or a
 * path with dots is never followed even if some day a bad value reaches the page.
 */
export function safeAppPath(link: string | null | undefined): string | null {
  if (typeof link !== 'string' || link.length > 300) return null
  return /^\/platform\/[A-Za-z0-9_\-/]+(\?[A-Za-z0-9_\-=&%.]*)?$/.test(link) && !link.includes('//') ? link : null
}
