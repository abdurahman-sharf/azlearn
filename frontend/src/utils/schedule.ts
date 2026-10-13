import type { ScheduleItem } from '@/api/platformStudent'
import { safeAppPath } from './safePath'

// Pure rules of "My schedule": the week window in the viewer's LOCAL time (day boundaries are local midnights, built from
// calendar fields so a daylight-saving change never shifts them), grouping by local day, the address-bar state, and when
// a live session can be joined. Kept out of the view so a plain script can test them.

/** A live session opens for joining this long before it starts (the same number the server's live page uses). */
export const JOIN_LEAD_MS = 15 * 60_000
export const SCHEDULE_DAYS = 7

/** Local midnight of the day containing `ms`. */
export function startOfDay(ms: number): number {
  const d = new Date(ms)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
}

/** Local midnight `n` calendar days after the day containing `ms` (a 23 or 25 hour day counts as one day). */
export function addDays(ms: number, n: number): number {
  const d = new Date(ms)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n).getTime()
}

const pad = (n: number, w = 2) => String(n).padStart(w, '0')

/** `YYYY-MM-DD` of the LOCAL day containing `ms`. */
export function dayKey(ms: number): string {
  const d = new Date(ms)
  return `${pad(d.getFullYear(), 4)}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** Local midnight of a `YYYY-MM-DD` key; null when it is not a real calendar date (or far outside any sensible range). */
export function parseDayKey(s: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
  if (!m) return null
  const y = Number(m[1]), mo = Number(m[2]), d = Number(m[3])
  if (y < 2000 || y > 2100) return null
  const t = new Date(y, mo - 1, d)
  if (t.getFullYear() !== y || t.getMonth() !== mo - 1 || t.getDate() !== d) return null
  return t.getTime()
}

export interface WeekWindow {
  /** local midnight of the first day */
  start: number
  /** local midnight after the last day (exclusive) */
  end: number
  /** the request's `from` / `to`: `to` is the last millisecond of the last day, so an item exactly at midnight belongs to ONE week */
  from: number
  to: number
}

export function weekWindow(startMs: number, days = SCHEDULE_DAYS): WeekWindow {
  const start = startOfDay(startMs)
  const end = addDays(start, days)
  return { start, end, from: start, to: end - 1 }
}

/** The first day of the window `weeks` windows away (negative = earlier). */
export const shiftWeek = (startMs: number, weeks: number, days = SCHEDULE_DAYS): number => addDays(startOfDay(startMs), weeks * days)

/** The window starts today unless the address names another day (`?start=YYYY-MM-DD`). */
export function weekFromQuery(query: Record<string, unknown>, now: number): number {
  const raw = Array.isArray(query.start) ? query.start[0] : query.start
  const parsed = typeof raw === 'string' ? parseDayKey(raw) : null
  return parsed ?? startOfDay(now)
}

/** The address for a window: nothing when it starts today. */
export function weekToQuery(start: number, now: number): Record<string, string> {
  return startOfDay(start) === startOfDay(now) ? {} : { start: dayKey(start) }
}

export const isTodayWindow = (start: number, now: number): boolean => startOfDay(start) === startOfDay(now)

export interface DayGroup {
  key: string
  /** local midnight of the day */
  start: number
  items: ScheduleItem[]
}

/**
 * Items sorted by time (then id) and grouped by the LOCAL day they start on; days without items are left out.
 *
 * `windowStart` (the local midnight of the first day on screen) is for a window that is fetched with the server's overlap
 * rule: a session that began before the window and is still running (it crossed midnight) is listed under the FIRST day
 * of the window rather than under a day that is not on screen at all. Without it every item stays on its own start day.
 */
export function groupByDay(items: readonly ScheduleItem[], windowStart?: number): DayGroup[] {
  const sorted = [...items].sort((a, b) => a.at - b.at || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0) || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0))
  const out: DayGroup[] = []
  for (const it of sorted) {
    const shownOn = windowStart !== undefined && it.at < windowStart ? windowStart : it.at
    const key = dayKey(shownOn)
    const last = out[out.length - 1]
    if (last && last.key === key) last.items.push(it)
    else out.push({ key, start: startOfDay(shownOn), items: [it] })
  }
  return out
}

/** When a live item ends (an exam entry is a point in time). */
export const itemEnd = (i: Pick<ScheduleItem, 'at' | 'ends_at'>): number => i.ends_at ?? i.at

export type LiveItemPhase = 'upcoming' | 'live_now' | 'ended'

/** Where a live item is now: before its start, between start and end, or over. */
export function livePhase(startsAt: number, endsAt: number, now: number): LiveItemPhase {
  return now > endsAt ? 'ended' : now >= startsAt ? 'live_now' : 'upcoming'
}

/** The joining window `[starts_at - 15 min, end]` (inclusive). */
export const canJoinNow = (startsAt: number, endsAt: number, now: number): boolean => now >= startsAt - JOIN_LEAD_MS && now <= endsAt

/**
 * The button of a live row: "Join" while the joining window is open, "Details" otherwise. Exam rows have neither. A session
 * whose link is withheld (`locked`: not enrolled in its subject) never offers "Join" - there is nothing to join from there;
 * its page explains and offers to enrol.
 */
export function rowJoinAction(i: Pick<ScheduleItem, 'kind' | 'at' | 'ends_at'> & { locked?: boolean }, now: number): 'join' | 'details' | null {
  if (i.kind !== 'live') return null
  return !i.locked && canJoinNow(i.at, itemEnd(i), now) ? 'join' : 'details'
}

/** The in-app path of a row, only if it is one (a path under /platform; anything else is dropped). */
export const safeLink = safeAppPath
