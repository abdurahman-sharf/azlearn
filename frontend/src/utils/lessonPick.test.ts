import { allDone, doneCount, embedSrc, externalHref, fileSrc, groupBySection, lessonFromQuery, nextLessonId, percentDone, pickInitialLesson, playerKind } from './lessonPick.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const L = (id: string, section: string | null = null) => ({ id, section })
const lessons = [L('a'), L('b'), L('c', 'Part 2'), L('d', 'Part 2'), L('e')]

// --- the opening lesson -----------------------------------------------------------------------------------------------
eq(pickInitialLesson(lessons, new Set()), 'a', 'nothing done: the first lesson')
eq(pickInitialLesson(lessons, new Set(['a', 'b'])), 'c', 'the first lesson not completed')
eq(pickInitialLesson(lessons, new Set(['a', 'c'])), 'b', 'a gap: the first one not completed, not the last one touched')
eq(pickInitialLesson(lessons, new Set(['a', 'b', 'c', 'd', 'e'])), 'a', 'everything done: back to the first')
eq(pickInitialLesson(lessons, new Set(['a']), 'd'), 'd', 'an address names the lesson')
eq(pickInitialLesson(lessons, new Set(['a']), 'zzz'), 'b', 'an unknown id in the address is ignored')
eq(pickInitialLesson(lessons, new Set(['a']), ''), 'b', 'an empty id is ignored')
eq(pickInitialLesson(lessons, new Set(['a', 'b']), null, false), 'a', 'not tracking (teacher, not enrolled): always the first')
eq(pickInitialLesson(lessons, new Set(['a', 'b']), 'e', false), 'e', 'not tracking, but the address still wins')
eq(pickInitialLesson([], new Set()), null, 'a course without lessons')
eq(pickInitialLesson(lessons, new Set(['gone', 'a'])), 'b', 'a mark for a deleted lesson changes nothing')

// --- next lesson ------------------------------------------------------------------------------------------------------------
eq(nextLessonId(lessons, 'a'), 'b', 'next in order')
eq(nextLessonId(lessons, 'b'), 'c', 'across a section boundary')
eq(nextLessonId(lessons, 'e'), null, 'the last lesson has no next')
eq(nextLessonId(lessons, 'nope'), null, 'unknown id')
eq(nextLessonId(lessons, null), null, 'no current lesson')
eq(nextLessonId([], 'a'), null, 'empty course')

// --- progress ---------------------------------------------------------------------------------------------------------------
eq(doneCount(lessons, new Set(['a', 'c', 'gone'])), 2, 'deleted lessons are not counted')
eq(percentDone(lessons, new Set(['a', 'b'])), 40, '2 of 5')
eq(percentDone(lessons, new Set(['a', 'b', 'c'])), 60, '3 of 5')
eq(percentDone([L('x'), L('y'), L('z')], new Set(['x'])), 33, 'rounded')
eq(percentDone([], new Set(['a'])), 0, 'no lessons: 0, never NaN')
eq(allDone(lessons, new Set(['a', 'b', 'c', 'd', 'e'])), true, 'all done')
eq(allDone(lessons, new Set(['a', 'b', 'c', 'd'])), false, 'one missing')
eq(allDone([], new Set()), false, 'an empty course is never "completed"')
eq(allDone(lessons, new Set(['a', 'b', 'c', 'd', 'gone'])), false, 'a mark for a deleted lesson is not a completed lesson')

// --- sections ---------------------------------------------------------------------------------------------------------------
eq(groupBySection(lessons).map((g) => [g.section, g.lessons.map((l) => l.id)]), [[null, ['a', 'b']], ['Part 2', ['c', 'd']], [null, ['e']]], 'consecutive grouping')
eq(groupBySection([]), [], 'no lessons, no groups')
eq(groupBySection([L('a', 'S'), L('b', 'T'), L('c', 'S')]).length, 3, 'a section that comes back later is a new group (order is the teacher\'s)')

// --- the address ------------------------------------------------------------------------------------------------------------
eq(lessonFromQuery({ lesson: 'abc' }), 'abc', 'plain')
eq(lessonFromQuery({ lesson: ['x', 'y'] }), 'x', 'repeated key: the first')
eq(lessonFromQuery({}), null, 'absent')
eq(lessonFromQuery({ lesson: '' }), null, 'empty')
eq(lessonFromQuery({ lesson: 'a'.repeat(65) }), null, 'too long')
eq(lessonFromQuery({ lesson: 5 }), null, 'not a string')

// --- the player -------------------------------------------------------------------------------------------------------------
const yt = { video_kind: 'youtube' as const, video_url: 'https://youtu.be/abcdefghijk', embed_url: 'https://www.youtube-nocookie.com/embed/abcdefghijk' }
const vm = { video_kind: 'vimeo' as const, video_url: 'https://vimeo.com/123', embed_url: 'https://player.vimeo.com/video/123' }
const file = { video_kind: 'file' as const, video_url: 'https://cdn.example/v.mp4', embed_url: 'https://cdn.example/v.mp4' }
const link = { video_kind: 'link' as const, video_url: 'https://example.com/watch', embed_url: null }
eq(playerKind(yt), 'embed', 'YouTube plays inside the page')
eq(playerKind(vm), 'embed', 'Vimeo plays inside the page')
eq(playerKind(file), 'file', 'a video file uses <video>')
eq(playerKind(file, true), 'link', 'a file that failed to load falls back to the link')
eq(playerKind(link), 'link', 'any other link is external')
eq(playerKind({ ...yt, locked: true, video_url: '', embed_url: null }), 'locked', 'a withheld lesson')
eq(embedSrc({ ...yt, embed_url: 'https://evil.example/embed/x' }), null, 'a server value outside the two providers is never framed')
eq(embedSrc({ ...yt, embed_url: 'http://www.youtube-nocookie.com/embed/abcdefghijk' }), null, 'http is never framed')
eq(embedSrc({ ...yt, embed_url: 'javascript:alert(1)' }), null, 'javascript: is never framed')
eq(embedSrc({ ...yt, embed_url: 'https://www.youtube-nocookie.com/embed/x" onload="y' }), null, 'no quotes')
eq(embedSrc({ ...link, embed_url: 'https://www.youtube-nocookie.com/embed/abcdefghijk' }), null, 'a link kind is never framed even with an embed address')
eq(embedSrc({ ...yt, locked: true }), null, 'locked: nothing to frame')
eq(playerKind({ ...yt, embed_url: null }), 'link', 'YouTube without an embed address: a link')
eq(fileSrc({ ...file, embed_url: null }), 'https://cdn.example/v.mp4', 'the file address falls back to the video url')
eq(fileSrc({ ...file, embed_url: 'http://cdn.example/v.mp4', video_url: 'http://cdn.example/v.mp4' }), null, 'http files are not played')
eq(externalHref(link), 'https://example.com/watch', 'https link offered')
eq(externalHref({ ...link, video_url: 'javascript:alert(1)' }), null, 'a non-https address is not offered')
eq(externalHref({ ...link, video_url: '', locked: true }), null, 'locked: no link')
console.log('lesson pick tests ok')
