import { brandCss, brandTokens, contrast, parseHex, type RGB } from './brandTheme.ts'

const WHITE: RGB = [255, 255, 255]
const DARK_SURFACE: RGB = [28, 27, 31]

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

{ // parseHex accepts only #RRGGBB
  eq(parseHex('#1A6CFF'), [26, 108, 255], 'hex')
  eq(parseHex(' #0b5d3b '), [11, 93, 59], 'hex trimmed')
  for (const bad of ['', '1A6CFF', '#12345', '#1234567', '#GGGGGG', 'red', '#ééé']) eq(parseHex(bad), null, `parseHex(${bad})`)
}

{ // contrast matches WCAG reference values
  ok(Math.abs(contrast([0, 0, 0], WHITE) - 21) < 0.01, 'black/white is 21:1')
  ok(Math.abs(contrast(WHITE, WHITE) - 1) < 1e-9, 'white/white is 1:1')
  ok(contrast([118, 118, 118], WHITE) >= 4.5 && contrast([119, 119, 119], WHITE) < 4.5, 'AA boundary grey')
}

// Every colour the server would accept (>= 4.5 against white) must yield readable tokens in BOTH themes.
{ // derived tokens reach WCAG AA for every admissible brand colour (hue/saturation/lightness sweep)
  const hsl = (h: number, s: number, l: number): RGB => {
    const a = s * Math.min(l, 1 - l)
    const f = (n: number) => { const k = (n + h / 30) % 12; return l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1)) }
    return [f(0), f(8), f(4)].map(v => Math.round(v * 255)) as RGB
  }
  let checked = 0
  for (let h = 0; h < 360; h += 5) for (const s of [0.1, 0.4, 0.7, 1]) for (const l of [0.08, 0.15, 0.25, 0.35, 0.45, 0.5]) {
    const rgb = hsl(h, s, l)
    if (contrast(rgb, WHITE) < 4.5) continue // the server rejects these
    const hex = '#' + rgb.map(v => v.toString(16).padStart(2, '0')).join('')
    const t = brandTokens(hex)!
    const where = `${hex} (h${h} s${s} l${l})`
    ok(contrast(t.light.onPrimary, t.light.primary) >= 4.5, `light primary/label ${where}`)
    ok(contrast(t.light.onContainer, t.light.container) >= 4.5, `light container ${where}`)
    ok(contrast(t.dark.primary, DARK_SURFACE) >= 4.5, `dark primary on surface ${where}`)
    ok(contrast(t.dark.onPrimary, t.dark.primary) >= 4.5, `dark label ${where}`)
    ok(contrast(t.dark.onContainer, t.dark.container) >= 4.5, `dark container ${where}`)
    checked++
  }
  ok(checked > 300, `sweep covered ${checked} colours`)
}

{ // pure black and the default blue are handled
  for (const hex of ['#000000', '#1A6CFF', '#0B5D3B', '#7A1F1F']) {
    const t = brandTokens(hex)!
    ok(contrast(t.dark.primary, DARK_SURFACE) >= 4.5, hex)
    ok(contrast(t.dark.onContainer, t.dark.container) >= 4.5, hex)
  }
}

{ // css only targets the default accent and never injects arbitrary text
  const css = brandCss('#0B5D3B')
  ok(/^:root\[data-accent="blue"\]:not\(\.dark\)\{--md-primary:11 93 59;/.test(css), 'light selector + primary triplet')
  ok(/\.dark\[data-accent="blue"\]\{/.test(css), 'dark selector')
  ok(css.includes('--md-secondary:11 93 59;') && css.includes('--md-secondary-container:'), 'secondary tokens follow the brand colour')
  eq(brandCss('red'), '', 'invalid colour yields no css')
  eq(brandCss('#000;}body{display:none'), '', 'css injection attempt yields no css')
  ok(brandCss('#0B5D3B').replace(/:root.*?\{.*?\}|\.dark.*?\{.*?\}/g, '') === '', 'nothing but the two generated rules')
}
