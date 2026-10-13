import { buildCalendar, buildLiveIcs, foldLine, icsEscape, icsUtc, liveToEvent } from './ics.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
function ok(c: boolean, msg: string): void {
  if (!c) throw new Error(msg)
}
const bytes = (s: string) => new TextEncoder().encode(s).length

// --- escaping -------------------------------------------------------------------------------------------------------------
eq(icsEscape('a,b;c\\d'), 'a\\,b\\;c\\\\d', 'comma, semicolon, backslash')
eq(icsEscape('line1\nline2'), 'line1\\nline2', 'newline')
eq(icsEscape('line1\r\nline2\rline3'), 'line1\\nline2\\nline3', 'every kind of line break')
eq(icsEscape('a\u0000b\u0007c\td\u007fe'), 'abc de', 'control characters are dropped, a tab becomes a space')
eq(icsEscape('x\r\nEND:VCALENDAR'), 'x\\nEND:VCALENDAR', 'a title cannot start a new property')
eq(icsEscape('مرحبا، عالم'), 'مرحبا، عالم', 'Arabic text (the Arabic comma is not a delimiter) is untouched')

// --- dates ----------------------------------------------------------------------------------------------------------------
eq(icsUtc(Date.UTC(2026, 9, 12, 7, 5, 9)), '20261012T070509Z', 'UTC, zero padded')
eq(icsUtc(Date.UTC(2026, 0, 1, 0, 0, 0)), '20260101T000000Z', 'year start')

// --- folding --------------------------------------------------------------------------------------------------------------
eq(foldLine('short line'), 'short line', 'short lines are left alone')
eq(foldLine('a'.repeat(75)), 'a'.repeat(75), 'exactly 75 octets stays on one line')
const folded = foldLine('a'.repeat(76))
eq(folded, 'a'.repeat(75) + '\r\n ' + 'a', '76 octets fold once')
const long = foldLine('b'.repeat(200))
for (const [i, part] of long.split('\r\n').entries()) {
  ok(bytes(part) <= 75, `physical line ${i} is ${bytes(part)} octets`)
  if (i > 0) ok(part.startsWith(' '), 'continuations start with one space')
}
eq(long.split('\r\n').join('').replace(/ /g, ''), 'b'.repeat(200), 'unfolding gives the original back')
// Arabic: two octets per letter; a line is never cut inside a character
const arabic = 'SUMMARY:' + 'درس مباشر في الرياضيات '.repeat(8)
const arFolded = foldLine(arabic)
for (const [i, part] of arFolded.split('\r\n').entries()) {
  ok(bytes(part) <= 75, `Arabic line ${i} is ${bytes(part)} octets`)
  ok(!part.includes('�'), 'no broken sequence')
}
eq(arFolded.split('\r\n').map((p, i) => (i ? p.slice(1) : p)).join(''), arabic, 'unfolding Arabic gives the original back')
// a four-octet emoji at the boundary moves whole to the next line
const emoji = foldLine('a'.repeat(73) + '😀' + 'b')
eq(emoji.split('\r\n')[0], 'a'.repeat(73), 'the emoji does not fit in the last 2 octets')
ok(emoji.split('\r\n')[1]!.startsWith(' 😀'), 'and starts the continuation line')

// --- a whole file ---------------------------------------------------------------------------------------------------------
const stamp = Date.UTC(2026, 9, 1, 12, 0, 0)
const file = buildCalendar(
  [{ uid: 'u1@azlearn', start: Date.UTC(2026, 9, 12, 10), end: Date.UTC(2026, 9, 12, 11), summary: 'A, B; C', description: 'one\ntwo', alarmMinutes: 15 }],
  { calName: 'azlearn', now: stamp },
)
eq(file, [
  'BEGIN:VCALENDAR', 'VERSION:2.0', 'PRODID:-//azlearn//schedule//AR', 'CALSCALE:GREGORIAN', 'X-WR-CALNAME:azlearn',
  'BEGIN:VEVENT', 'UID:u1@azlearn', 'DTSTAMP:20261001T120000Z', 'DTSTART:20261012T100000Z', 'DTEND:20261012T110000Z', 'SUMMARY:A\\, B\\; C', 'DESCRIPTION:one\\ntwo',
  'BEGIN:VALARM', 'ACTION:DISPLAY', 'DESCRIPTION:A\\, B\\; C', 'TRIGGER:-PT15M', 'END:VALARM', 'END:VEVENT', 'END:VCALENDAR', '',
].join('\r\n'), 'golden file')
ok(!/[^\r]\n/.test(file) && !/\r[^\n]/.test(file), 'CRLF everywhere, no bare LF or CR')
ok(file.endsWith('\r\n'), 'ends with a line break')
const noAlarm = buildCalendar([{ uid: 'u', start: 1, end: 2, summary: 's' }], { calName: 'c', now: stamp })
ok(!noAlarm.includes('VALARM') && !noAlarm.includes('DESCRIPTION'), 'no alarm and no description unless asked')
const zero = buildCalendar([{ uid: 'u', start: 60_000, end: 60_000, summary: 's' }], { calName: 'c', now: stamp })
ok(zero.includes('DTEND:19700101T000200Z'), 'a zero-length event still ends after it starts')
eq(buildCalendar([], { calName: 'c', now: stamp }).split('\r\n').length, 7, 'an empty calendar is still a valid file')

// --- one live session -----------------------------------------------------------------------------------------------------
const labels = { calName: 'My azlearn schedule', teacher: 'Teacher', subject: 'Subject', join: 'Join link' }
const live = {
  id: 'L1', title: 'Algebra, part 2', description: 'Bring a pen', teacher_name: 'Sara', subject_name: 'Maths',
  starts_at: Date.UTC(2026, 9, 12, 10), duration_min: 45, join_url: 'https://meet.example/room?x=1&y=2', locked: false,
}
const ev = liveToEvent(live, labels)
eq([ev.uid, ev.start, ev.end, ev.alarmMinutes], ['live-L1@azlearn', live.starts_at, live.starts_at + 45 * 60_000, 15], 'uid, times, 15-minute alarm')
ok(ev.description!.includes('https://meet.example/room?x=1&y=2'), 'the join link is included when unlocked')
const lockedEv = liveToEvent({ ...live, locked: true, join_url: '' }, labels)
ok(!lockedEv.description!.includes('https://') && !lockedEv.description!.includes('Join link'), 'locked: no join line')
const leaky = liveToEvent({ ...live, locked: true }, labels)
ok(!leaky.description!.includes('meet.example'), 'locked wins even if a URL slipped through')
const notHttps = liveToEvent({ ...live, join_url: 'javascript:alert(1)' }, labels)
ok(!notHttps.description!.includes('javascript'), 'only https links go into the file')
const text = buildLiveIcs(live, labels, stamp)
ok(text.includes('SUMMARY:Algebra\\, part 2\r\n'), 'the title is escaped')
ok(text.includes('DTSTART:20261012T100000Z\r\n') && text.includes('DTEND:20261012T104500Z\r\n'), 'start and end in UTC')
ok(text.includes('TRIGGER:-PT15M\r\n'), 'the alarm')
for (const line of text.split('\r\n')) ok(bytes(line) <= 75, `line over 75 octets: ${line}`)
const arText = buildLiveIcs({ ...live, title: 'درس الرياضيات المباشر: المعادلات التربيعية والأسس واللوغاريتمات', teacher_name: 'أ. سارة' }, labels, stamp)
for (const line of arText.split('\r\n')) ok(bytes(line) <= 75, `Arabic line over 75 octets: ${line}`)
console.log('ics tests ok')
