import {
  JOIN_LEAD_MS, addDays, canJoinNow, dayKey, groupByDay, isTodayWindow, itemEnd, livePhase, parseDayKey, rowJoinAction, safeLink, shiftWeek, startOfDay,
  weekFromQuery, weekToQuery, weekWindow,
} from './schedule.ts'
import type { ScheduleItem } from '@/api/platformStudent'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// Local-time helpers: every expectation is built from calendar fields, so the test passes in any time zone.
const at = (y: number, mo: number, d: number, h = 0, mi = 0) => new Date(y, mo - 1, d, h, mi).getTime()

// --- days -----------------------------------------------------------------------------------------------------------------
eq(startOfDay(at(2026, 10, 12, 15, 30)), at(2026, 10, 12), 'midnight of the same local day')
eq(startOfDay(at(2026, 10, 12, 0, 0)), at(2026, 10, 12), 'midnight is its own start')
eq(dayKey(at(2026, 1, 5, 23, 59)), '2026-01-05', 'local key, zero padded')
eq(dayKey(at(2026, 12, 31, 0, 0)), '2026-12-31', 'year end')
eq(addDays(at(2026, 10, 12, 18), 1), at(2026, 10, 13), 'next day midnight')
eq(addDays(at(2026, 10, 31), 1), at(2026, 11, 1), 'across a month end')
eq(addDays(at(2026, 12, 28), 7), at(2027, 1, 4), 'across a year end')
eq(addDays(at(2026, 3, 1), -1), at(2026, 2, 28), 'backwards over February')
eq(addDays(at(2028, 2, 28), 1), at(2028, 2, 29), 'leap day')
// across any daylight-saving change the day count still advances by exactly one calendar day
for (const m of [3, 4, 10, 11]) for (let d = 1; d <= 28; d++) {
  const start = at(2026, m, d)
  eq(dayKey(addDays(start, 1)), dayKey(at(2026, m, d + 1)), `day after ${dayKey(start)}`)
  eq(new Date(addDays(start, 7)).getHours(), 0, `a week after ${dayKey(start)} is still midnight`)
}

// --- keys -----------------------------------------------------------------------------------------------------------------
eq(parseDayKey('2026-10-12'), at(2026, 10, 12), 'a real date')
eq(parseDayKey('2026-02-30'), null, 'not a real date')
eq(parseDayKey('2026-13-01'), null, 'month 13')
eq(parseDayKey('2026-1-1'), null, 'unpadded')
eq(parseDayKey('1999-12-31'), null, 'far in the past')
eq(parseDayKey('2101-01-01'), null, 'far ahead')
eq(parseDayKey('2026-10-12T00:00'), null, 'extra text')
eq(parseDayKey('2028-02-29'), at(2028, 2, 29), 'leap day is real')
eq(parseDayKey('2027-02-29'), null, 'not a leap year')

// --- the week window ------------------------------------------------------------------------------------------------------
const now = at(2026, 10, 12, 15, 30)
const w = weekWindow(now)
eq([w.start, w.end], [at(2026, 10, 12), at(2026, 10, 19)], 'seven local days from today')
eq([w.from, w.to], [at(2026, 10, 12), at(2026, 10, 19) - 1], 'the request ends one millisecond before the next window starts')
eq(shiftWeek(w.start, 1), at(2026, 10, 19), 'next week')
eq(shiftWeek(w.start, -1), at(2026, 10, 5), 'previous week')
eq(shiftWeek(at(2026, 10, 26), 1), at(2026, 11, 2), 'next week across a month end')
eq(weekWindow(at(2026, 3, 25)).end, at(2026, 4, 1), 'a window over the March clock change ends at local midnight')
eq(weekFromQuery({}, now), at(2026, 10, 12), 'no address: today')
eq(weekFromQuery({ start: '2026-11-02' }, now), at(2026, 11, 2), 'a named start')
eq(weekFromQuery({ start: 'garbage' }, now), at(2026, 10, 12), 'garbage: today')
eq(weekFromQuery({ start: ['2026-11-02', 'x'] }, now), at(2026, 11, 2), 'repeated key: the first')
eq(weekToQuery(at(2026, 10, 12), now), {}, 'today leaves the address clean')
eq(weekToQuery(at(2026, 10, 19), now), { start: '2026-10-19' }, 'another week is named')
eq(weekFromQuery(weekToQuery(at(2026, 11, 9), now), now), at(2026, 11, 9), 'round trip')
eq(isTodayWindow(at(2026, 10, 12), now), true, 'the window that starts today')
eq(isTodayWindow(at(2026, 10, 19), now), false, 'another window')

// --- grouping -------------------------------------------------------------------------------------------------------------
const item = (p: Partial<ScheduleItem> & { id: string; at: number }): ScheduleItem => ({
  kind: 'live', title: p.id, subject_id: 's', subject_name: 'S', teacher_name: 'T', ends_at: p.at + 3_600_000, duration_min: 60, locked: false, link: `/platform/live/${p.id}`, ...p,
})
const groups = groupByDay([
  item({ id: 'c', at: at(2026, 10, 14, 9) }),
  item({ id: 'a', at: at(2026, 10, 12, 18, 30) }),
  item({ id: 'b', at: at(2026, 10, 12, 8) }),
  item({ id: 'd', at: at(2026, 10, 14, 9) }),
  item({ id: 'x', kind: 'exam_opens', at: at(2026, 10, 14, 23, 59), ends_at: null, duration_min: null, link: '/platform/assessments/x' }),
])
eq(groups.map((g) => [g.key, g.items.map((i) => i.id)]), [['2026-10-12', ['b', 'a']], ['2026-10-14', ['c', 'd', 'x']]], 'sorted by time then id; days without items are left out')
eq(groups[0]!.start, at(2026, 10, 12), 'the group knows its local midnight')
eq(groupByDay([]), [], 'nothing, no groups')
eq(groupByDay([item({ id: 'late', at: at(2026, 10, 12, 23, 59) }), item({ id: 'early', at: at(2026, 10, 13, 0, 0) })]).map((g) => g.key), ['2026-10-12', '2026-10-13'], 'midnight belongs to the new day')

// a session that began before the window (it crossed midnight) is listed under the window's first day, not under a day off screen
const winStart = at(2026, 10, 12)
const carried = groupByDay([
  item({ id: 'over', at: at(2026, 10, 11, 23, 30), ends_at: at(2026, 10, 12, 0, 30) }),
  item({ id: 'mon', at: at(2026, 10, 12, 9) }),
  item({ id: 'tue', at: at(2026, 10, 13, 9) }),
], winStart)
eq(carried.map((g) => [g.key, g.items.map((i) => i.id)]), [['2026-10-12', ['over', 'mon']], ['2026-10-13', ['tue']]], 'the carried-over session leads the first day')
eq(carried[0]!.start, winStart, 'and the group is the first day')
eq(groupByDay([item({ id: 'over', at: at(2026, 10, 11, 23, 30) })]).map((g) => g.key), ['2026-10-11'], 'without a window start every item stays on its own day')
eq(groupByDay([item({ id: 'edge', at: winStart })], winStart).map((g) => g.key), ['2026-10-12'], 'an item exactly at the window start is on that day')

// --- joining --------------------------------------------------------------------------------------------------------------
const start = at(2026, 10, 12, 10), end = at(2026, 10, 12, 11)
eq(JOIN_LEAD_MS, 900_000, 'fifteen minutes')
eq(canJoinNow(start, end, start - JOIN_LEAD_MS - 1), false, '1 ms before the window')
eq(canJoinNow(start, end, start - JOIN_LEAD_MS), true, 'exactly 15 minutes before')
eq(canJoinNow(start, end, start), true, 'at the start')
eq(canJoinNow(start, end, end), true, 'at the end')
eq(canJoinNow(start, end, end + 1), false, '1 ms after the end')
eq(livePhase(start, end, start - 1), 'upcoming', 'before')
eq(livePhase(start, end, start), 'live_now', 'at the start')
eq(livePhase(start, end, end), 'live_now', 'at the end')
eq(livePhase(start, end, end + 1), 'ended', 'after')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end }), start - 60_000), 'join', 'inside the window: Join')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end }), start - 2 * JOIN_LEAD_MS), 'details', 'too early: Details')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end }), end + 5), 'details', 'over: Details')
eq(rowJoinAction(item({ id: 'x', kind: 'exam_closes', at: start, ends_at: null }), start), null, 'exam rows have no join button')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end, locked: true }), start - 60_000), 'details', 'locked (not enrolled): never "Join", even inside the window')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end, locked: true }), start - 2 * JOIN_LEAD_MS), 'details', 'locked and early: Details')
eq(rowJoinAction(item({ id: 'a', at: start, ends_at: end, locked: false }), start), 'join', 'unlocked, inside the window: Join')
eq(itemEnd({ at: 5, ends_at: null }), 5, 'a point in time ends where it starts')
eq(itemEnd({ at: 5, ends_at: 9 }), 9, 'a live session ends at its end')

// --- links ----------------------------------------------------------------------------------------------------------------
eq(safeLink('/platform/live/abc-123'), '/platform/live/abc-123', 'an in-app path')
eq(safeLink('/platform/assessments/x1'), '/platform/assessments/x1', 'an exam path')
eq(safeLink('https://evil.example/platform/x'), null, 'absolute URLs are dropped')
eq(safeLink('//evil.example'), null, 'protocol-relative URLs are dropped')
eq(safeLink('/other/place'), null, 'only /platform paths')
eq(safeLink('/platform/../x'), null, 'no dots')
eq(safeLink(null), null, 'nothing')
console.log('schedule tests ok')
