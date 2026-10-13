import { safeAppPath } from './safePath'

// Pure rules of the platform search page: when a query may be sent, which groups a role sees, how a hit's text is split
// for highlighting (never HTML - the view prints the pieces as plain text), and how "show all" pages are merged.
// The Arabic folding mirrors the server's (`norm_text` / the `ar_fold` SQL function): lower-case, tashkeel and tatweel
// removed, the alef forms folded to a bare alef, alef maqsura to ya. The server decides what matches; this only finds
// WHERE in the text to highlight, so it must fold the same way. Keep the two in step.

export type SearchGroup = 'subjects' | 'teachers' | 'courses' | 'posts' | 'live' | 'exams'
export const SEARCH_GROUPS: SearchGroup[] = ['subjects', 'teachers', 'courses', 'posts', 'live', 'exams']
/** The shortest and longest query the server accepts (else 400 `invalid_query`). */
export const MIN_QUERY = 2
export const MAX_QUERY = 60
/** Rows the first answer carries per group, and the page size of "show all". */
export const FIRST_PAGE = 8
export const MORE_PAGE = 20
export const DEBOUNCE_MS = 300

/** Exams exist for students only (the server leaves the group empty for everyone else). */
export function groupsFor(role: string | null | undefined): SearchGroup[] {
  return role === 'student' ? SEARCH_GROUPS : SEARCH_GROUPS.filter((g) => g !== 'exams')
}

/** Trimmed, inner white space collapsed, capped at the server's limit. */
export function normalizeQuery(q: string): string {
  return q.replace(/\s+/g, ' ').trim().slice(0, MAX_QUERY)
}

/** A query may be sent from two characters on (counted in characters, not UTF-16 units). */
export const queryValid = (q: string): boolean => [...normalizeQuery(q)].length >= MIN_QUERY

/** The folded form of one character (`''` for the marks the server drops). */
function foldChar(c: string): string {
  const cp = c.codePointAt(0)!
  if ((cp >= 0x064b && cp <= 0x065f) || cp === 0x0670 || cp === 0x0640) return ''
  if (c === 'أ' || c === 'إ' || c === 'آ' || c === 'ٱ') return 'ا'
  if (c === 'ى') return 'ي'
  return c.toLowerCase()
}

/** The folded text and, for each folded UTF-16 unit, the index in the original text where its character starts. */
export function foldWithMap(text: string): { folded: string; map: number[] } {
  let folded = ''
  const map: number[] = []
  let i = 0
  for (const ch of text) {
    const f = foldChar(ch)
    for (let k = 0; k < f.length; k++) map.push(i)
    folded += f
    i += ch.length
  }
  return { folded, map }
}

export const foldText = (s: string): string => foldWithMap(s).folded

export interface TextPart { text: string; match: boolean }

/** At most this many highlighted pieces per text (a query of "a" in a long text must not create thousands of nodes). */
const MAX_MARKS = 20

/**
 * Splits `text` into pieces, marking the parts that match the query after folding. Matches the whole phrase first; when the
 * phrase is nowhere in the text, each word of two or more characters is tried instead. The pieces always concatenate back to
 * `text` exactly, so printing them as text (never as HTML) shows the original.
 */
export function highlightParts(text: string, query: string): TextPart[] {
  const q = foldText(normalizeQuery(query))
  if (!text || !q) return [{ text, match: false }]
  const { folded, map } = foldWithMap(text)
  const find = (needle: string): [number, number][] => {
    const hits: [number, number][] = []
    let from = 0
    while (hits.length < MAX_MARKS) {
      const at = folded.indexOf(needle, from)
      if (at < 0) break
      hits.push([at, at + needle.length])
      from = at + needle.length
    }
    return hits
  }
  let spans = find(q)
  if (!spans.length) {
    const words = [...new Set(q.split(' ').filter((w) => [...w].length >= MIN_QUERY))]
    spans = words.flatMap(find).sort((a, b) => a[0] - b[0])
  }
  if (!spans.length) return [{ text, match: false }]
  // folded index -> original index; the end of a span is the end of the last matched character
  const startOf = (fi: number) => map[fi] ?? text.length
  const endOf = (fe: number) => {
    const lastStart = map[fe - 1] ?? text.length - 1
    const cp = text.codePointAt(lastStart)
    let end = lastStart + (cp !== undefined && cp > 0xffff ? 2 : 1)
    // the marks that belong to the last letter (its tashkeel) are part of the match
    while (end < text.length && foldChar(String.fromCodePoint(text.codePointAt(end)!)) === '') end += text.codePointAt(end)! > 0xffff ? 2 : 1
    return end
  }
  const out: TextPart[] = []
  let pos = 0
  for (const [fs, fe] of spans) {
    const s = Math.max(startOf(fs), pos)
    const e = endOf(fe)
    if (e <= s) continue
    if (s > pos) out.push({ text: text.slice(pos, s), match: false })
    out.push({ text: text.slice(s, e), match: true })
    pos = e
  }
  if (pos < text.length) out.push({ text: text.slice(pos), match: false })
  return out
}

interface WithId { id: string }

/** "Show all" answers come after the rows already shown: append them, dropping any id that is already there. */
export function mergeHits<T extends WithId>(existing: readonly T[], incoming: readonly T[]): T[] {
  const seen = new Set(existing.map((h) => h.id))
  const out = [...existing]
  for (const h of incoming) {
    if (!seen.has(h.id)) {
      seen.add(h.id)
      out.push(h)
    }
  }
  return out
}

/** The group has more matches than are listed (a button offers the rest). */
export const hasMoreHits = (count: number | null | undefined, loaded: number): boolean => (count ?? 0) > loaded

/** The sum of the group counts, for the summary line. */
export function totalCount(counts: Partial<Record<SearchGroup, number>> | null | undefined, groups: readonly SearchGroup[]): number {
  if (!counts) return 0
  return groups.reduce((n, g) => n + (counts[g] ?? 0), 0)
}

/** An in-app path to link a hit to (a hit link from the server; anything else is not linked). */
export const safeHitLink = safeAppPath
