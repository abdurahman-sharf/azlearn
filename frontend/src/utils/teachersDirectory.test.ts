import {
  DEFAULT_TEACHER_FILTERS, teacherClampPage, teacherFiltered, teacherFiltersFromQuery, teacherFiltersToQuery, teacherListQuery, teacherPages, withFollowing,
} from './teachersDirectory.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(teacherFiltersToQuery(DEFAULT_TEACHER_FILTERS), {}, 'defaults leave the address clean')
eq(teacherFiltersToQuery({ q: ' سارة ', subjectId: 's1', page: 2 }), { q: 'سارة', subject: 's1', page: '3' }, 'only what differs, one-based page')
eq(teacherFiltersFromQuery({ q: 'x', subject: 's2', page: '2' }), { q: 'x', subjectId: 's2', page: 1 }, 'read back')
eq(teacherFiltersFromQuery({ page: '0' }), DEFAULT_TEACHER_FILTERS, 'page 0 is the first page')
eq(teacherFiltersFromQuery({ q: ['a', 'b'], page: ['-4'] }), { ...DEFAULT_TEACHER_FILTERS, q: 'a' }, 'repeated keys use the first, a negative page is ignored')
eq(teacherFiltersFromQuery({ q: 'a'.repeat(500) }).q.length, 100, 'search text is capped')
eq(teacherFiltersFromQuery({ page: 'abc' }).page, 0, 'garbage page')
eq(teacherFiltersFromQuery({ page: '99999999' }).page, 10_000, 'page capped')
eq(teacherFiltersFromQuery(teacherFiltersToQuery({ q: 'q', subjectId: 'z', page: 5 })), { q: 'q', subjectId: 'z', page: 5 }, 'round trip')

eq(teacherListQuery(DEFAULT_TEACHER_FILTERS), { limit: 24, offset: 0 }, 'no filters')
eq(teacherListQuery({ q: '  q ', subjectId: 's', page: 2 }), { limit: 24, offset: 48, q: 'q', subject_id: 's' }, 'search, subject, page')
eq(teacherListQuery({ q: '', subjectId: '', page: -3 }).offset, 0, 'a negative page is the first')
eq(teacherListQuery({ q: '', subjectId: '', page: 1 }, 10).offset, 10, 'custom page size')

eq(teacherPages(0), 1, 'an empty list has one page')
eq(teacherPages(24), 1, 'exactly one full page')
eq(teacherPages(25), 2, 'one more row: a second page')
eq(teacherClampPage(9, 49), 2, 'the last page that exists')
eq(teacherClampPage(-1, 49), 0, 'never below the first')

const rows = [
  { id: 'a', full_name: 'A', bio: null, subject_count: 1, following: false },
  { id: 'b', full_name: 'B', bio: 'x', subject_count: 2, following: true },
]
const after = withFollowing(rows, 'a', true)
eq(after.map((t) => t.following), [true, true], 'the toggled row changed')
eq(rows[0]!.following, false, 'the original list is untouched')
eq(after[1] === rows[1], true, 'other rows are the same objects')
eq(withFollowing(rows, 'zzz', true).map((t) => t.following), [false, true], 'an unknown id changes nothing')

eq(teacherFiltered(DEFAULT_TEACHER_FILTERS), false, 'default is not filtered')
eq(teacherFiltered({ q: '  ', subjectId: '', page: 3 }), false, 'a blank search and a page are not a filter')
eq(teacherFiltered({ q: 'x', subjectId: '', page: 0 }), true, 'search')
eq(teacherFiltered({ q: '', subjectId: 's', page: 0 }), true, 'subject')
console.log('teachers directory tests ok')
