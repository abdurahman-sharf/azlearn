// Arabic text coming out of PDFs. PDF producers store glyphs in *visual* order, shaped as presentation forms
// (U+FB50–FDFF, U+FE70–FEFF); naive extraction therefore returns reversed, unreadable lines (and, joined with spaces,
// one letter per "word"). This module restores logical Arabic in two ways:
//
//  - `assembleArabicPage` — for pdf.js text items: groups them into lines by position and re-orders right-to-left
//    runs geometrically (exact; no heuristics about the language);
//  - `fixVisualOrder` — for plain strings (the Cloudflare worker's parser): the same string algorithm as
//    packages/core/src/parser/arabic.rs (article statistics decide whether the text is reversed).
//
// Keep `fixVisualOrder` in sync with arabic.rs and workers/src/arabicPdf.ts.

export interface PdfTextItem {
  str: string
  dir?: string
  /** pdf.js transform: [a, b, c, d, x, y]; x/y are the item's origin in page space. */
  transform: number[]
  width: number
  height: number
  hasEOL?: boolean
}

const PRESENTATION = /[\uFB50-\uFDFF\uFE70-\uFEFF]/
const ARABIC_LETTER = /[\u0620-\u064A\u066E-\u06D3\u06FA-\u06FF\u0750-\u077F\u08A0-\u08C9\uFB50-\uFDFF\uFE70-\uFEFF]/
const STARTS_WITH_MARK = /^[\u064B-\u065F\u0670]/
const LTR_RUN_CHAR = /[A-Za-z0-9\u0660-\u0669\u06F0-\u06F9]/
const JOINER = /[.,:/\-%_]/

export function hasArabic(s: string): boolean {
  return ARABIC_LETTER.test(s)
}

/** Presentation forms → base letters; a ligature (one glyph, several letters) expands in reverse when `forVisual`. */
export function toBaseLetters(s: string, forVisual: boolean): string {
  let out = ''
  for (const ch of s) {
    if (!PRESENTATION.test(ch)) {
      out += ch
      continue
    }
    let parts = [...ch.normalize('NFKC')]
    if (forVisual && parts.length > 1) parts = parts.reverse()
    out += parts.join('')
  }
  return out
}

function reverseLine(line: string): string {
  const rev = [...line].reverse()
  let out = ''
  let i = 0
  while (i < rev.length) {
    if (LTR_RUN_CHAR.test(rev[i]!)) {
      const start = i
      while (i < rev.length && (LTR_RUN_CHAR.test(rev[i]!) || (JOINER.test(rev[i]!) && i + 1 < rev.length && LTR_RUN_CHAR.test(rev[i + 1]!)))) i++
      out += rev.slice(start, i).reverse().join('')
    } else {
      out += rev[i]
      i++
    }
  }
  return out
}

function arabicShare(line: string): number {
  let arabic = 0
  let letters = 0
  for (const c of line) {
    if (ARABIC_LETTER.test(c)) {
      arabic++
      letters++
    } else if (/\p{L}/u.test(c)) letters++
  }
  return letters === 0 ? 0 : arabic / letters
}

/** One space where there were several; none before a vowel mark (it belongs to the letter before) or between a lone alef and the lam after it. */
export function tidyLine(s: string): string {
  const out: string[] = []
  for (const tok of s.split(' ').filter((t) => t)) {
    const prev = out[out.length - 1]
    if (prev !== undefined && STARTS_WITH_MARK.test(tok)) out[out.length - 1] = prev + tok
    else if (prev === 'ا' && tok.startsWith('ل')) out[out.length - 1] = prev + tok
    else out.push(tok)
  }
  return (s.startsWith(' ') ? ' ' : '') + out.join(' ')
}

function articleEvidence(text: string): [number, number] {
  let forward = 0
  let reversed = 0
  for (const w of text.split(/[^\p{L}\u064B-\u065F\u0670]+/u)) {
    const letters = [...w].filter((c) => ARABIC_LETTER.test(c))
    if (letters.length < 4) continue
    const s = (a: string) => letters.slice(0, a.length).join('') === a
    const e = (a: string) => letters.slice(letters.length - a.length).join('') === a
    const first = letters[0]!
    const last = letters[letters.length - 1]!
    const prefix = 'وبفكل'
    if (s('ال') || (prefix.includes(first) && letters.slice(1, 3).join('') === 'ال') || s('لل')) forward++
    if (e('لا') || (prefix.includes(last) && letters.slice(letters.length - 3, letters.length - 1).join('') === 'لا') || e('لل')) reversed++
  }
  return [forward, reversed]
}

/** Converts Arabic text extracted from a PDF (visual order, presentation forms) to logical order. */
export function fixVisualOrder(text: string): string {
  if (!hasArabic(text)) return text
  const hadPresentation = PRESENTATION.test(text)
  const logical = toBaseLetters(text, false)
  const [forward, reversed] = articleEvidence(logical)
  const isReversed = forward + reversed >= 3 ? reversed > forward : hadPresentation
  if (!isReversed) return logical
  return toBaseLetters(text, true)
    .split('\n')
    .map((l) => (arabicShare(l) >= 0.5 ? tidyLine(reverseLine(l)) : l))
    .join('\n')
}

// ───────── pdf.js items ─────────

type Kind = 'R' | 'L' | 'N'

function kindOf(it: PdfTextItem): Kind {
  if (it.dir === 'rtl' || hasArabic(it.str)) return 'R'
  if (/[A-Za-z0-9]/.test(it.str)) return 'L'
  return 'N' // spaces and punctuation take the direction of their neighbours
}

/**
 * Text of one page from pdf.js items, in reading order, with Arabic restored. Items are grouped into lines by their
 * vertical position and sorted left to right; for a right-to-left line the runs are then emitted from the right,
 * while numbers and Latin words keep their left-to-right order. Each item's own text is already in logical order
 * (pdf.js applies the Unicode bidi algorithm inside an item), so nothing inside an item is reversed.
 */
export function assembleArabicPage(items: PdfTextItem[]): string {
  const real = items.filter((i) => typeof i.str === 'string' && i.str !== '')
  if (!real.length) return ''
  const heights = real.map((i) => i.height).filter((h) => h > 0).sort((a, b) => a - b)
  const medianH = heights.length ? heights[Math.floor(heights.length / 2)]! : 10
  const tol = Math.max(3, medianH * 0.5)

  const byY = [...real].sort((a, b) => b.transform[5]! - a.transform[5]!)
  const lines: PdfTextItem[][] = []
  for (const it of byY) {
    const line = lines[lines.length - 1]
    if (line && Math.abs(line[0]!.transform[5]! - it.transform[5]!) <= tol) line.push(it)
    else lines.push([it])
  }

  return lines
    .map((line) => {
      line.sort((a, b) => a.transform[4]! - b.transform[4]!)
      // a gap wider than a third of the text height is a word break, unless a space item already says so
      const gapAfter = line.map((it, i) => {
        const next = line[i + 1]
        if (!next) return false
        const gap = next.transform[4]! - (it.transform[4]! + it.width)
        return gap > Math.max(it.height, next.height, 1) * 0.33 && !it.str.endsWith(' ') && !next.str.startsWith(' ')
      })
      const text = (idx: number[]) => {
        let s = ''
        idx.forEach((k, n) => {
          s += line[k]!.str
          const nextK = idx[n + 1]
          if (nextK !== undefined && Math.abs(nextK - k) === 1 ? gapAfter[Math.min(k, nextK)] : nextK !== undefined) s += ' '
        })
        return s
      }
      const all = line.map((_, i) => i)
      const kinds = line.map(kindOf)
      const strong = line.filter((it) => /\p{L}/u.test(it.str))
      const arabicLetters = strong.filter((it) => hasArabic(it.str)).length
      if (!strong.length || arabicLetters / strong.length < 0.5) return tidyLine(toBaseLetters(text(all), false)).trim()

      // runs of the same direction (neutral items stay with the run they follow; a leading neutral joins the first)
      const runs: { kind: 'R' | 'L'; idx: number[] }[] = []
      let pendingNeutral: number[] = []
      all.forEach((i) => {
        const k = kinds[i]!
        if (k === 'N') {
          if (runs.length) runs[runs.length - 1]!.idx.push(i)
          else pendingNeutral.push(i)
          return
        }
        const last = runs[runs.length - 1]
        if (last && last.kind === k) last.idx.push(i)
        else runs.push({ kind: k, idx: [...pendingNeutral.splice(0), i] })
      })
      if (pendingNeutral.length) runs.push({ kind: 'R', idx: pendingNeutral })
      const order: number[] = []
      for (const r of runs.reverse()) order.push(...(r.kind === 'R' ? [...r.idx].reverse() : r.idx))
      return tidyLine(toBaseLetters(text(order), false)).trim()
    })
    .filter((l) => l)
    .join('\n')
}
