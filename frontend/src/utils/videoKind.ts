// How a lesson link will behave for students, decided in the browser BEFORE saving. It mirrors the server rule
// (`parse_video` / `https_url` in packages/server/src/platform_content.rs): keep the two in step. The server stays the
// authority - it recomputes the embed address itself and never trusts what the client says; this only lets the editor
// say "plays inside the page" or "opens in a new tab" next to the field.

export type VideoKind = 'youtube' | 'vimeo' | 'file' | 'link'

/** Same limit as the server's MAX_URL_CHARS. */
export const MAX_URL_CHARS = 500

/**
 * The server's `https_url`: only https links, a host, no credentials, no whitespace or control characters, at most
 * MAX_URL_CHARS characters. Returns the parsed address, or null when the server would answer 400 invalid_url.
 */
export function parseHttpsUrl(raw: string): URL | null {
  const t = raw.trim()
  if (!t || [...t].length > MAX_URL_CHARS) return null
  // \s covers the Unicode white space Rust's char::is_whitespace knows; the second class is the C0/C1 control characters
  if (/\s/u.test(t) || /[\u0000-\u001f\u007f-\u009f]/u.test(t)) return null
  let u: URL
  try {
    u = new URL(t)
  } catch {
    return null
  }
  if (u.protocol !== 'https:' || !u.hostname || u.username || u.password) return null
  return u
}

/** True when the server would accept this as a link (https only). */
export const isHttpsUrl = (raw: string): boolean => parseHttpsUrl(raw) !== null

// A video id: letters, digits, `_` and `-`; `len` is exact for YouTube (11), otherwise at most 20.
function validVideoId(id: string, len?: number): boolean {
  return id.length > 0 && (len === undefined ? id.length <= 20 : id.length === len) && /^[A-Za-z0-9_-]+$/.test(id)
}

/**
 * `null` = not a valid https link at all (the server answers 400 invalid_url). Otherwise the kind of the link:
 * `youtube`/`vimeo`/`file` are shown inside the page, `link` is only offered as an external link.
 */
export function classifyVideoUrl(raw: string): VideoKind | null {
  const u = parseHttpsUrl(raw)
  if (!u) return null
  // trim_start_matches strips the prefix repeatedly, like the server does
  let host = u.hostname.toLowerCase()
  while (host.startsWith('www.')) host = host.slice(4)
  while (host.startsWith('m.')) host = host.slice(2)
  const segs = u.pathname.split('/').filter((s) => s !== '')
  switch (host) {
    case 'youtube.com': {
      const first = segs[0]
      const id = first === 'watch' ? u.searchParams.get('v') : first === 'embed' || first === 'shorts' || first === 'live' ? segs[1] : undefined
      return id != null && validVideoId(id, 11) ? 'youtube' : 'link'
    }
    case 'youtu.be':
      return segs[0] !== undefined && validVideoId(segs[0], 11) ? 'youtube' : 'link'
    case 'vimeo.com':
      return segs[0] !== undefined && /^[0-9]+$/.test(segs[0]) && validVideoId(segs[0]) ? 'vimeo' : 'link'
    default: {
      const path = u.pathname.toLowerCase()
      return ['.mp4', '.webm', '.ogg'].some((e) => path.endsWith(e)) ? 'file' : 'link'
    }
  }
}

/** Whether students see the lesson inside the page (an iframe or a video element) rather than as an external link. */
export const embedsInPage = (kind: VideoKind | null): boolean => kind === 'youtube' || kind === 'vimeo' || kind === 'file'
