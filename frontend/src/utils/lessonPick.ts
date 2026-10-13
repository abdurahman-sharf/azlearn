import { parseHttpsUrl } from './videoKind'

// Pure rules of the course page: which lesson it opens on, which one comes next, how far the student is, and how the
// lessons are grouped under their section headings. Kept out of the view so a plain script can test them.
// The server returns the lessons already ordered (position, then creation); nothing here re-sorts them.

export interface LessonRef {
  id: string
  section: string | null
}

export interface LessonGroup<T extends LessonRef> {
  section: string | null
  lessons: T[]
}

/** Consecutive lessons with the same section heading form one group (an unsectioned stretch has `section: null`). */
export function groupBySection<T extends LessonRef>(lessons: readonly T[]): LessonGroup<T>[] {
  const out: LessonGroup<T>[] = []
  for (const l of lessons) {
    const last = out[out.length - 1]
    if (last && last.section === l.section) last.lessons.push(l)
    else out.push({ section: l.section, lessons: [l] })
  }
  return out
}

/**
 * The lesson a course opens on: the one named in the address (`?lesson=`) when it exists in this course, else - for a student
 * who tracks progress - the first lesson not completed yet, else (everything done, or nobody is tracking) the first lesson.
 * `null` only for a course without lessons. A completed id that no longer exists in the course is ignored.
 */
export function pickInitialLesson(
  lessons: readonly LessonRef[],
  done: ReadonlySet<string>,
  requested?: string | null,
  tracking = true,
): string | null {
  if (!lessons.length) return null
  if (requested && lessons.some((l) => l.id === requested)) return requested
  if (tracking) {
    const open = lessons.find((l) => !done.has(l.id))
    if (open) return open.id
  }
  return lessons[0]!.id
}

/** The lesson right after `currentId` in the course order; `null` on the last lesson (or when `currentId` is unknown). */
export function nextLessonId(lessons: readonly LessonRef[], currentId: string | null | undefined): string | null {
  if (!currentId) return null
  const i = lessons.findIndex((l) => l.id === currentId)
  return i >= 0 && i + 1 < lessons.length ? lessons[i + 1]!.id : null
}

/** How many of the course's lessons are completed (marks for lessons that were deleted since do not count). */
export function doneCount(lessons: readonly LessonRef[], done: ReadonlySet<string>): number {
  let n = 0
  for (const l of lessons) if (done.has(l.id)) n++
  return n
}

/** Whole-number percent 0..100 (0 for a course without lessons). */
export function percentDone(lessons: readonly LessonRef[], done: ReadonlySet<string>): number {
  return lessons.length ? Math.round((doneCount(lessons, done) / lessons.length) * 100) : 0
}

/** Every lesson is completed (never true for a course without lessons). */
export function allDone(lessons: readonly LessonRef[], done: ReadonlySet<string>): boolean {
  return lessons.length > 0 && doneCount(lessons, done) === lessons.length
}

/** The lesson id in an address query (`?lesson=`), or null; a repeated key uses the first. */
export function lessonFromQuery(query: Record<string, unknown>): string | null {
  const v = query.lesson
  const s = Array.isArray(v) ? v[0] : v
  return typeof s === 'string' && s.length > 0 && s.length <= 64 ? s : null
}

// ── the player ───────────────────────────────────────────────────────────────────────────────────────────────────────

/** The only addresses ever put into an iframe: the server's embed forms for YouTube and Vimeo. Anything else is a link. */
const EMBED_PREFIXES = ['https://www.youtube-nocookie.com/embed/', 'https://player.vimeo.com/video/']

export interface PlayableLesson {
  video_kind: 'youtube' | 'vimeo' | 'file' | 'link'
  video_url: string
  embed_url: string | null
  locked?: boolean
}

/** The iframe address for a YouTube / Vimeo lesson, or null (never a teacher-typed URL: only the server's embed form qualifies). */
export function embedSrc(l: PlayableLesson): string | null {
  if (l.locked || !l.embed_url) return null
  if (l.video_kind !== 'youtube' && l.video_kind !== 'vimeo') return null
  const e = l.embed_url
  return EMBED_PREFIXES.some((p) => e.startsWith(p)) && /^https:\/\/[^\s"'<>]+$/.test(e) ? e : null
}

/** The address of a direct video file (https only), or null. */
export function fileSrc(l: PlayableLesson): string | null {
  if (l.locked || l.video_kind !== 'file') return null
  return parseHttpsUrl(l.embed_url || l.video_url)?.href ?? null
}

/** The external link for "open the video" (https only: a link that is not is not offered). */
export function externalHref(l: PlayableLesson): string | null {
  return l.locked ? null : parseHttpsUrl(l.video_url)?.href ?? null
}

export type PlayerKind = 'locked' | 'embed' | 'file' | 'link'

/** What the player area shows: the withheld notice, an embedded provider player, a <video> for a file, or an external link. */
export function playerKind(l: PlayableLesson, fileFailed = false): PlayerKind {
  if (l.locked) return 'locked'
  if (embedSrc(l)) return 'embed'
  if (!fileFailed && fileSrc(l)) return 'file'
  return 'link'
}
