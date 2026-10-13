import type { ContentStatusFilter, HiddenReason } from '@/api/platformContent'

// Small pure rules of the teacher's "My content" hub, kept out of the view so a plain script can test them.

export type HubTab = 'all' | 'draft' | 'published' | 'hidden'
/** The hub's type filter: the three lists the server pages, plus the exams (a separate, unpaged list filtered here). */
export type HubType = '' | 'post' | 'course' | 'live' | 'exam'

/** The `status` query value for a tab (`all` = no filter). */
export function statusForTab(tab: HubTab): ContentStatusFilter | undefined {
  return tab === 'all' ? undefined : tab
}

/**
 * Where "why is this hidden?" leads. Only a missing assignment is something the teacher can act on (ask for the subject
 * again on My subjects); a switched-off subject or institution, or an inactive account, is the admin's decision and gets
 * an explanation only.
 */
export function hiddenReasonLink(reason: HiddenReason | null | undefined): string | null {
  return reason === 'no_assignment' ? '/platform/teaching' : null
}

/** Pages needed to show the biggest of the per-type totals when each type shows `per` items per page (at least 1). */
export function pageCount(totals: number[], per: number): number {
  if (per <= 0) return 1
  return Math.max(1, ...totals.map((t) => Math.ceil(Math.max(0, t) / per)))
}

/**
 * The page to show: `page` itself while it exists, else the last page that does. `totals` are ALL the lists that share the
 * pager (the server's per-type totals AND the exams, which are paged here): the pager runs to the longest of them, so a
 * page that only the exams still fill is a valid page even though the server lists are empty on it.
 */
export function clampPage(page: number, totals: number[], per: number): number {
  return Math.min(Math.max(0, page), pageCount(totals, per) - 1)
}

/** Server offset of a zero-based page. */
export const pageOffset = (page: number, per: number): number => Math.max(0, page) * Math.max(1, per)

/**
 * A copy's title: the original plus a localized suffix such as " (copy)", cut so the whole stays within the server's
 * title limit (the original loses its tail, never the suffix). The caller decides the language of the suffix.
 */
export function copyTitle(title: string, suffix: string, max = 200): string {
  const base = title.trim()
  if ([...base].length + [...suffix].length <= max) return base + suffix
  return [...base].slice(0, Math.max(0, max - [...suffix].length)).join('').trimEnd() + suffix
}

/** Fields the hub's client-side filter needs from an exam row. */
export interface ExamFilterRow {
  title: string
  subject_id: string
  status: 'draft' | 'published' | 'closed' | 'archived'
  visible?: boolean
  hidden_reason?: HiddenReason | null
}

/**
 * The hub's filters applied to the exams list (the server does not page or filter `/assessments/mine`). Same meaning as
 * the server's filters for the other types: `draft` = not published yet; `published` = stored status published (even if
 * it is currently hidden - the row then carries the hidden chip); `hidden` = out (published/closed) but not shown to
 * students. Closed and archived exams therefore only appear under "all" (and "hidden" when something hides them).
 */
export function filterExams<T extends ExamFilterRow>(rows: T[], f: { q: string; tab: HubTab; subjectId: string }): T[] {
  const q = f.q.trim().toLowerCase()
  return rows.filter((e) => {
    if (q && !e.title.toLowerCase().includes(q)) return false
    if (f.subjectId && e.subject_id !== f.subjectId) return false
    switch (f.tab) {
      case 'draft': return e.status === 'draft'
      case 'published': return e.status === 'published'
      case 'hidden': return isHiddenFromStudents(e)
      default: return true
    }
  })
}

/**
 * The exams in the order the hub's sort asks for. `updated` keeps the server's order (latest change first); `title` is A-Z
 * in the viewer's language. The whole filtered list is ordered BEFORE it is cut into pages. (The server orders the other
 * lists itself, through its `sort` parameter, for the same reason.)
 */
export function sortExams<T extends { title: string }>(rows: T[], sort: 'updated' | 'title', locale?: string): T[] {
  if (sort !== 'title') return rows
  return [...rows].sort((a, b) => a.title.localeCompare(b.title, locale) || 0)
}

// Statuses in which an item is "out" for students (a draft, a cancelled session and an archived exam are the teacher's own choice).
const OUT = new Set(['published', 'scheduled', 'closed'])

/** What an owner's row needs to say how it stands with students. */
export interface OwnerRow {
  status: string
  visible?: boolean
  hidden_reason?: HiddenReason | null
  /** live sessions: the time is over */
  ended?: boolean
}

/**
 * Whether an owner's row should show the "hidden: <reason>" chip: it is out (published/scheduled/closed), the server says
 * students cannot see it, and something stands in the way (`hidden_reason`). `visible: false` with an explicit `null`
 * reason is the normal state of a closed exam or an ended session, not a problem. A server that sends no reason field
 * at all still gets the chip for `visible: false` (nothing to name, but the owner must know).
 */
export function isHiddenFromStudents(row: OwnerRow): boolean {
  return OUT.has(row.status) && row.visible === false && row.hidden_reason !== null
}

export type OwnerState = 'hidden' | 'draft' | 'published' | 'scheduled' | 'cancelled' | 'ended' | 'closed' | 'archived'

/** The one state chip of an owner's row. Hidden wins over published/scheduled/closed, an ended session over everything but cancelled. */
export function ownerState(row: OwnerRow): OwnerState {
  switch (row.status) {
    case 'draft': return 'draft'
    case 'cancelled': return 'cancelled'
    case 'archived': return 'archived'
  }
  if (row.ended) return 'ended'
  if (isHiddenFromStudents(row)) return 'hidden'
  if (row.status === 'closed') return 'closed'
  if (row.status === 'scheduled') return 'scheduled'
  return 'published'
}

// --- the hub's filters in the address bar ---------------------------------------------------------------------------------

export type HubSort = 'updated' | 'title'
export interface HubFilters {
  q: string
  tab: HubTab
  type: HubType
  subjectId: string
  sort: HubSort
  /** zero-based */
  page: number
}
export const DEFAULT_HUB_FILTERS: HubFilters = { q: '', tab: 'all', type: '', subjectId: '', sort: 'updated', page: 0 }

const TABS: HubTab[] = ['all', 'draft', 'published', 'hidden']
const TYPES: HubType[] = ['', 'post', 'course', 'live', 'exam']
const SORTS: HubSort[] = ['updated', 'title']
const first = (v: unknown): string => (Array.isArray(v) ? String(v[0] ?? '') : typeof v === 'string' ? v : '')

/** Reads the filters back from a route query; anything unknown or malformed falls back to the default. */
export function filtersFromQuery(query: Record<string, unknown>): HubFilters {
  const tab = first(query.tab) as HubTab
  const type = first(query.type) as HubType
  const sort = first(query.sort) as HubSort
  const page = Number.parseInt(first(query.page), 10)
  return {
    q: first(query.q).slice(0, 100),
    tab: TABS.includes(tab) ? tab : 'all',
    type: TYPES.includes(type) ? type : '',
    subjectId: first(query.subject).slice(0, 64),
    sort: SORTS.includes(sort) ? sort : 'updated',
    page: Number.isFinite(page) && page > 0 ? Math.min(page - 1, 10_000) : 0,
  }
}

/** The route query for the filters: only what differs from the default, the page one-based. */
export function filtersToQuery(f: HubFilters): Record<string, string> {
  const out: Record<string, string> = {}
  if (f.q.trim()) out.q = f.q.trim()
  if (f.tab !== 'all') out.tab = f.tab
  if (f.type) out.type = f.type
  if (f.subjectId) out.subject = f.subjectId
  if (f.sort !== 'updated') out.sort = f.sort
  if (f.page > 0) out.page = String(f.page + 1)
  return out
}

/** True when any filter is on (the empty state then offers "clear filters" instead of "create your first item"). */
export const hasActiveFilters = (f: HubFilters): boolean => !!f.q.trim() || f.tab !== 'all' || !!f.type || !!f.subjectId
