// A calendar file (RFC 5545) built in the browser for ONE live lesson ("add to my calendar" on the live page). The server
// builds the whole timetable file itself with the same rules (CRLF line ends, lines folded at 75 octets, TEXT escaped,
// UTC times, a 15-minute alarm); keep the two in step. Free text is data: it is escaped and stripped of control characters
// so a title can never inject a property, and the join link is only included when the viewer may see it.

const CRLF = '\r\n'

/** Control characters other than the line break and the tab (a tab becomes a space first). */
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g

/** TEXT value escaping: backslash, semicolon, comma and line breaks (as `\n`); control characters are dropped. */
export function icsEscape(text: string): string {
  return text
    .replace(/\r\n|\r|\n/g, '\n')
    .replace(/\t/g, ' ')
    .replace(CONTROL, '')
    .replace(/\\/g, '\\\\')
    .replace(/;/g, '\\;')
    .replace(/,/g, '\\,')
    .replace(/\n/g, '\\n')
}

/** UTF-8 length of one code point. */
function octets(cp: number): number {
  return cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4
}

/**
 * Folds one content line: no physical line is longer than 75 octets, a continuation starts with one space (which counts
 * toward its 75), and a UTF-8 sequence is never cut in the middle (Arabic letters are two octets each).
 */
export function foldLine(line: string): string {
  const parts: string[] = []
  let cur = ''
  let len = 0
  let limit = 75
  for (const ch of line) {
    const n = octets(ch.codePointAt(0)!)
    if (len + n > limit) {
      parts.push(cur)
      cur = ''
      len = 1 // the leading space of the continuation line
      limit = 75
    }
    cur += ch
    len += n
  }
  parts.push(cur)
  return parts.join(CRLF + ' ')
}

/** `YYYYMMDDTHHMMSSZ` (UTC) for an epoch-ms instant. */
export function icsUtc(ms: number): string {
  const d = new Date(ms)
  const p = (n: number, w = 2) => String(n).padStart(w, '0')
  return `${p(d.getUTCFullYear(), 4)}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}T${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}Z`
}

export interface IcsEvent {
  uid: string
  start: number
  end: number
  summary: string
  description?: string
  /** a display alarm this many minutes before the start */
  alarmMinutes?: number
}

export interface IcsOptions {
  calName: string
  /** DTSTAMP (epoch ms); defaults to now */
  now?: number
  prodId?: string
}

/** The whole file: CRLF line ends (including after the last line), folded, escaped. */
export function buildCalendar(events: readonly IcsEvent[], opts: IcsOptions): string {
  const stamp = icsUtc(opts.now ?? Date.now())
  const lines: string[] = [
    'BEGIN:VCALENDAR',
    'VERSION:2.0',
    `PRODID:${opts.prodId ?? '-//azlearn//schedule//AR'}`,
    'CALSCALE:GREGORIAN',
    `X-WR-CALNAME:${icsEscape(opts.calName)}`,
  ]
  for (const e of events) {
    // an event never ends before it starts (a zero-length block would be dropped by some calendars)
    const end = e.end > e.start ? e.end : e.start + 60_000
    lines.push('BEGIN:VEVENT', `UID:${icsEscape(e.uid)}`, `DTSTAMP:${stamp}`, `DTSTART:${icsUtc(e.start)}`, `DTEND:${icsUtc(end)}`, `SUMMARY:${icsEscape(e.summary)}`)
    if (e.description) lines.push(`DESCRIPTION:${icsEscape(e.description)}`)
    if (e.alarmMinutes && e.alarmMinutes > 0) {
      lines.push('BEGIN:VALARM', 'ACTION:DISPLAY', `DESCRIPTION:${icsEscape(e.summary)}`, `TRIGGER:-PT${Math.round(e.alarmMinutes)}M`, 'END:VALARM')
    }
    lines.push('END:VEVENT')
  }
  lines.push('END:VCALENDAR')
  return lines.map(foldLine).join(CRLF) + CRLF
}

/** What the live page knows about a session (the student shape of `GET /live/{id}`). */
export interface LiveForIcs {
  id: string
  title: string
  description: string | null
  teacher_name: string
  subject_name: string
  starts_at: number
  duration_min: number
  join_url: string
  locked: boolean
}

export interface LiveIcsLabels {
  calName: string
  teacher: string
  subject: string
  join: string
}

const isHttps = (u: string) => /^https:\/\/\S+$/.test(u)

/** One live session as a calendar event with a 15-minute alarm; the join link only when the viewer is allowed to see it. */
export function liveToEvent(live: LiveForIcs, labels: LiveIcsLabels): IcsEvent {
  const lines = [`${labels.teacher}: ${live.teacher_name}`, `${labels.subject}: ${live.subject_name}`]
  if (live.description) lines.push('', live.description)
  if (!live.locked && live.join_url && isHttps(live.join_url)) lines.push('', `${labels.join}: ${live.join_url}`)
  return {
    uid: `live-${live.id}@azlearn`,
    start: live.starts_at,
    end: live.starts_at + Math.max(1, live.duration_min) * 60_000,
    summary: live.title,
    description: lines.join('\n'),
    alarmMinutes: 15,
  }
}

export function buildLiveIcs(live: LiveForIcs, labels: LiveIcsLabels, now?: number): string {
  return buildCalendar([liveToEvent(live, labels)], { calName: labels.calName, now })
}
