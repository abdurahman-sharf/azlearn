// Derives the Material-style colour tokens for a platform brand colour (admin-chosen `#RRGGBB`).
// Pure and dependency-free so it can be unit-tested with plain Node. The server only accepts brand
// colours that read with white text (WCAG AA); every derived pair here is built to reach AA as well.

export type RGB = [number, number, number]

const WHITE: RGB = [255, 255, 255]
const BLACK: RGB = [0, 0, 0]
const DARK_SURFACE: RGB = [28, 27, 31] // --md-surface in dark mode

export function parseHex(hex: string): RGB | null {
  const m = /^#([0-9a-fA-F]{6})$/.exec(hex.trim())
  if (!m) return null
  const n = parseInt(m[1]!, 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

export function luminance([r, g, b]: RGB): number {
  const lin = (c: number) => {
    const v = c / 255
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

export function contrast(a: RGB, b: RGB): number {
  const [la, lb] = [luminance(a), luminance(b)]
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

const mix = (a: RGB, b: RGB, t: number): RGB => [0, 1, 2].map(i => Math.round(a[i]! + (b[i]! - a[i]!) * t)) as RGB

/** Moves `c` toward `target` in small steps until it reaches `min` contrast against `against`. */
function until(c: RGB, target: RGB, against: RGB, min: number, from = 0): RGB {
  for (let t = from; t <= 1.0001; t += 0.04) {
    const candidate = mix(c, target, Math.min(t, 1))
    if (contrast(candidate, against) >= min) return candidate
  }
  return target
}

export interface Tokens {
  primary: RGB
  onPrimary: RGB
  container: RGB
  onContainer: RGB
}

export function brandTokens(hex: string): { light: Tokens; dark: Tokens } | null {
  const c = parseHex(hex)
  if (!c) return null
  // Light: the brand colour itself carries white labels; container is a pale tint with a deep shade on top.
  const lightContainer = mix(c, WHITE, 0.86)
  const light: Tokens = {
    primary: c,
    onPrimary: WHITE,
    container: lightContainer,
    onContainer: until(c, BLACK, lightContainer, 7, 0.55),
  }
  // Dark: a lightened brand colour on the dark surface, dark text on it, and a deep container with pale text.
  const darkPrimary = until(c, WHITE, DARK_SURFACE, 4.5, 0.35)
  const darkContainer = mix(c, BLACK, 0.6)
  const dark: Tokens = {
    primary: darkPrimary,
    onPrimary: until(c, BLACK, darkPrimary, 4.5, 0.7),
    container: darkContainer,
    onContainer: until(c, WHITE, darkContainer, 7, 0.8),
  }
  return { light, dark }
}

const triplet = (c: RGB) => c.join(' ')
const block = (t: Tokens) =>
  `--md-primary:${triplet(t.primary)};--md-on-primary:${triplet(t.onPrimary)};` +
  `--md-primary-container:${triplet(t.container)};--md-on-primary-container:${triplet(t.onContainer)};` +
  `--md-secondary:${triplet(t.primary)};--md-on-secondary:${triplet(t.onPrimary)};` +
  `--md-secondary-container:${triplet(t.container)};--md-on-secondary-container:${triplet(t.onContainer)};`

/**
 * CSS applied only while the user keeps the default accent (`data-accent="blue"`): a personal accent
 * choice in Settings always wins over the platform colour.
 */
export function brandCss(hex: string): string {
  const t = brandTokens(hex)
  if (!t) return ''
  return `:root[data-accent="blue"]:not(.dark){${block(t.light)}}.dark[data-accent="blue"]{${block(t.dark)}}`
}
