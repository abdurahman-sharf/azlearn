import {
  DEFAULT_LEARNING_FILTERS, continueLink, learningClampPage, learningFiltersFromQuery, learningFiltersToQuery, learningListQuery, learningTabCount,
  progressView, rowAction,
} from './learning.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- the address bar and the request ------------------------------------------------------------------------------------
eq(learningFiltersToQuery(DEFAULT_LEARNING_FILTERS), {}, 'defaults leave the address clean')
eq(learningFiltersToQuery({ tab: 'completed', q: ' algebra ', subjectId: 's1', page: 2 }), { q: 'algebra', tab: 'completed', subject: 's1', page: '3' }, 'only what differs, one-based page')
eq(learningFiltersFromQuery({ tab: 'in_progress', q: 'x', subject: 's2', page: '2' }), { tab: 'in_progress', q: 'x', subjectId: 's2', page: 1 }, 'read back')
eq(learningFiltersFromQuery({ tab: 'open', page: '0' }), DEFAULT_LEARNING_FILTERS, 'an exams tab or page 0 is not ours: defaults')
eq(learningFiltersFromQuery({ tab: ['not_started', 'completed'], page: ['-1'] }), { ...DEFAULT_LEARNING_FILTERS, tab: 'not_started' }, 'repeated keys use the first, negative page ignored')
eq(learningFiltersFromQuery({ q: 'a'.repeat(300) }).q.length, 100, 'search text is capped')
eq(learningFiltersFromQuery({ page: 'abc' }).page, 0, 'garbage page')
eq(learningFiltersFromQuery({ page: '99999999' }).page, 10_000, 'page capped')
eq(learningFiltersFromQuery(learningFiltersToQuery({ tab: 'not_started', q: 'q', subjectId: 'z', page: 5 })), { tab: 'not_started', q: 'q', subjectId: 'z', page: 5 }, 'round trip')
eq(learningListQuery(DEFAULT_LEARNING_FILTERS), { limit: 24, offset: 0 }, 'all: no state sent')
eq(learningListQuery({ tab: 'in_progress', q: '  q ', subjectId: 's', page: 1 }), { limit: 24, offset: 24, state: 'in_progress', q: 'q', subject_id: 's' }, 'state, search, subject, page')
eq(learningListQuery({ ...DEFAULT_LEARNING_FILTERS, page: -3 }).offset, 0, 'negative page is the first')

// --- counts and paging --------------------------------------------------------------------------------------------------
const counts = { in_progress: 2, not_started: 5, completed: 1 }
eq(learningTabCount('all', counts), 8, 'all is the sum')
eq(learningTabCount('completed', counts), 1, 'one state')
eq(learningTabCount('completed', null), 0, 'unknown counts')
eq(learningTabCount('all', { completed: 3 }), 3, 'missing states count as zero')
eq(learningClampPage(4, 49), 2, 'the last page that exists')
eq(learningClampPage(0, 0), 0, 'empty list')
eq(learningClampPage(1, 24), 0, 'exactly one full page')
eq(learningClampPage(1, 25), 1, 'one more row: a second page')

// --- where a row leads --------------------------------------------------------------------------------------------------
eq(continueLink({ course_id: 'c1', next_lesson_id: 'l7' }), '/platform/courses/c1?lesson=l7', 'continue on the next lesson')
eq(continueLink({ course_id: 'c1', next_lesson_id: null }), '/platform/courses/c1', 'completed: the course itself')
eq(continueLink({ course_id: 'c1', next_lesson_id: 'a b&c' }), '/platform/courses/c1?lesson=a%20b%26c', 'the id is encoded')
eq(rowAction({ state: 'completed' }), 'review', 'completed')
eq(rowAction({ state: 'in_progress' }), 'continue', 'in progress')
eq(rowAction({ state: 'not_started' }), 'start', 'not started')

// --- progress -----------------------------------------------------------------------------------------------------------
eq(progressView({ completed: 2, total: 5, percent: 40 }), { percent: 40, label: '2 / 5' }, 'plain')
eq(progressView({ completed: 1, total: 3, percent: 33.33 }), { percent: 33, label: '1 / 3' }, 'rounded')
eq(progressView({ completed: 9, total: 3, percent: 300 }).percent, 100, 'clamped high')
eq(progressView({ completed: 0, total: 3, percent: -5 }).percent, 0, 'clamped low')
eq(progressView({ completed: 0, total: 0, percent: Number.NaN }).percent, 0, 'NaN is 0')
console.log('learning tests ok')
