import { clampPage, copyTitle, DEFAULT_HUB_FILTERS, filterExams, filtersFromQuery, filtersToQuery, hasActiveFilters, hiddenReasonLink, isHiddenFromStudents, ownerState, pageCount, pageOffset, sortExams, statusForTab, type ExamFilterRow } from './contentHub.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- tabs -> server status ---------------------------------------------------------------------------------------
eq(statusForTab('all'), undefined, 'all = no filter')
eq(statusForTab('draft'), 'draft', 'draft')
eq(statusForTab('published'), 'published', 'published')
eq(statusForTab('hidden'), 'hidden', 'hidden')

// --- reasons ----------------------------------------------------------------------------------------------------
eq(hiddenReasonLink('no_assignment'), '/platform/teaching', 'a missing assignment is fixed on My subjects')
eq(hiddenReasonLink('subject_inactive'), null, 'admin decision: explanation only')
eq(hiddenReasonLink('institution_inactive'), null, 'admin decision: explanation only')
eq(hiddenReasonLink('account_inactive'), null, 'account: explanation only')
eq(hiddenReasonLink(null), null, 'no reason')
eq(hiddenReasonLink(undefined), null, 'no reason field')

// --- who gets the "hidden" chip ---------------------------------------------------------------------------------
eq(isHiddenFromStudents({ status: 'published', visible: false }), true, 'published but not visible')
eq(isHiddenFromStudents({ status: 'scheduled', visible: false }), true, 'scheduled but not visible')
eq(isHiddenFromStudents({ status: 'closed', visible: false }), true, 'closed exam not visible')
eq(isHiddenFromStudents({ status: 'published', visible: true }), false, 'visible')
eq(isHiddenFromStudents({ status: 'published' }), false, 'older server without the flag')
eq(isHiddenFromStudents({ status: 'draft', visible: false }), false, 'a draft is simply a draft')
eq(isHiddenFromStudents({ status: 'cancelled', visible: false }), false, 'a cancelled session is not "hidden"')
eq(isHiddenFromStudents({ status: 'archived', visible: false }), false, 'an archived exam is the teacher\'s choice')
eq(isHiddenFromStudents({ status: 'published', visible: false, hidden_reason: 'no_assignment' }), true, 'a named reason')
eq(isHiddenFromStudents({ status: 'closed', visible: false, hidden_reason: null }), false, 'a closed exam that nothing hides is simply closed')
eq(isHiddenFromStudents({ status: 'closed', visible: false, hidden_reason: 'subject_inactive' }), true, 'a closed exam in a switched-off subject')

// --- the one state chip -----------------------------------------------------------------------------------------
eq(ownerState({ status: 'draft' }), 'draft', 'draft')
eq(ownerState({ status: 'draft', visible: false, hidden_reason: 'no_assignment' }), 'draft', 'a draft is never "hidden"')
eq(ownerState({ status: 'published', visible: true, hidden_reason: null }), 'published', 'published and visible')
eq(ownerState({ status: 'published', visible: false, hidden_reason: 'institution_inactive' }), 'hidden', 'published but hidden')
eq(ownerState({ status: 'scheduled', visible: true, hidden_reason: null }), 'scheduled', 'scheduled session')
eq(ownerState({ status: 'scheduled', visible: false, hidden_reason: 'account_inactive' }), 'hidden', 'scheduled but hidden')
eq(ownerState({ status: 'scheduled', visible: false, hidden_reason: null, ended: true }), 'ended', 'an ended session')
eq(ownerState({ status: 'scheduled', visible: false, hidden_reason: 'no_assignment', ended: true }), 'ended', 'ended wins over hidden')
eq(ownerState({ status: 'cancelled', ended: true }), 'cancelled', 'cancelled wins over ended')
eq(ownerState({ status: 'closed', visible: true }), 'closed', 'closed exam')
eq(ownerState({ status: 'closed', visible: false, hidden_reason: 'subject_inactive' }), 'hidden', 'closed exam in a switched-off subject')
eq(ownerState({ status: 'archived', visible: false, hidden_reason: null }), 'archived', 'archived exam')
eq(ownerState({ status: 'published' }), 'published', 'older server without the fields')

// --- paging -----------------------------------------------------------------------------------------------------
eq(pageCount([0, 0, 0], 20), 1, 'nothing = one page')
eq(pageCount([20, 5, 0], 20), 1, 'exactly one full page')
eq(pageCount([21, 5, 0], 20), 2, 'one over')
eq(pageCount([5, 61, 40], 20), 4, 'the biggest type decides')
eq(pageCount([], 20), 1, 'no types')
eq(pageCount([10], 0), 1, 'a bad page size does not divide by zero')
eq(pageCount([-5], 20), 1, 'negative totals are ignored')
eq(pageOffset(0, 20), 0, 'first page')
eq(pageOffset(3, 20), 60, 'fourth page')
eq(pageOffset(-1, 20), 0, 'negative page')

// --- copy titles ------------------------------------------------------------------------------------------------
eq(copyTitle('Algebra', ' (copy)'), 'Algebra (copy)', 'short title')
eq(copyTitle('  Algebra  ', ' (copy)'), 'Algebra (copy)', 'surrounding spaces trimmed')
const long = 'x'.repeat(200)
const cut = copyTitle(long, ' (copy)')
eq([...cut].length, 200, 'never longer than the limit')
eq(cut.endsWith(' (copy)'), true, 'the suffix survives')
eq(copyTitle('حساب التفاضل', ' (نسخة)'), 'حساب التفاضل (نسخة)', 'Arabic')
eq([...copyTitle('ب'.repeat(300), ' (نسخة)', 50)].length <= 50, true, 'custom limit')

// --- exam filtering ---------------------------------------------------------------------------------------------
const ex = (title: string, subject_id: string, status: ExamFilterRow['status'], visible?: boolean): ExamFilterRow => ({ title, subject_id, status, visible })
const exams = [
  ex('Midterm Algebra', 's1', 'published', true), ex('Final Algebra', 's1', 'draft', false), ex('Geometry quiz', 's2', 'published', false),
  ex('Old quiz', 's2', 'closed', true), ex('Archived quiz', 's2', 'archived', false), ex('Closed hidden', 's1', 'closed', false),
]
const titles = (f: Parameters<typeof filterExams>[1]) => filterExams(exams, f).map((e) => e.title)
eq(titles({ q: '', tab: 'all', subjectId: '' }), exams.map((e) => e.title), 'all keeps everything')
eq(titles({ q: '', tab: 'draft', subjectId: '' }), ['Final Algebra'], 'draft tab')
eq(titles({ q: '', tab: 'published', subjectId: '' }), ['Midterm Algebra', 'Geometry quiz'], 'published tab: stored status published, like the server (a hidden one still carries its chip)')
eq(titles({ q: '', tab: 'hidden', subjectId: '' }), ['Geometry quiz', 'Closed hidden'], 'hidden tab: out but not visible')
eq(titles({ q: 'ALGEBRA', tab: 'all', subjectId: '' }), ['Midterm Algebra', 'Final Algebra'], 'search ignores case')
eq(titles({ q: '  quiz ', tab: 'all', subjectId: '' }), ['Geometry quiz', 'Old quiz', 'Archived quiz'], 'search trims')
eq(titles({ q: '', tab: 'all', subjectId: 's2' }), ['Geometry quiz', 'Old quiz', 'Archived quiz'], 'subject filter')
eq(titles({ q: 'quiz', tab: 'hidden', subjectId: 's2' }), ['Geometry quiz'], 'filters combine')
eq(titles({ q: 'nothing', tab: 'all', subjectId: '' }), [], 'no match')

// a closed exam with an explicit "nothing hides it" is not hidden
const calm = [ex('Calm closed', 's1', 'closed', false)].map((e) => ({ ...e, hidden_reason: null }))
eq(filterExams(calm, { q: '', tab: 'hidden', subjectId: '' }).length, 0, 'closed + visible:false + null reason is not hidden')

// --- filters <-> route query ------------------------------------------------------------------------------------
eq(filtersFromQuery({}), DEFAULT_HUB_FILTERS, 'empty query = defaults')
eq(filtersToQuery(DEFAULT_HUB_FILTERS), {}, 'defaults write nothing')
const f = { q: ' algebra ', tab: 'hidden' as const, type: 'course' as const, subjectId: 's9', sort: 'title' as const, page: 2 }
eq(filtersToQuery(f), { q: 'algebra', tab: 'hidden', type: 'course', subject: 's9', sort: 'title', page: '3' }, 'non-defaults are written, page one-based')
eq(filtersFromQuery(filtersToQuery(f)), { ...f, q: 'algebra' }, 'round trip')
eq(filtersFromQuery({ tab: 'nope', type: 'x', sort: 'y', page: '-3' }), DEFAULT_HUB_FILTERS, 'unknown values fall back')
eq(filtersFromQuery({ page: 'abc' }).page, 0, 'bad page')
eq(filtersFromQuery({ page: '1' }).page, 0, 'page 1 is index 0')
eq(filtersFromQuery({ tab: ['draft', 'published'] }).tab, 'draft', 'a repeated parameter uses the first')
eq(filtersFromQuery({ q: 'x'.repeat(300) }).q.length, 100, 'search text is capped')
eq(hasActiveFilters(DEFAULT_HUB_FILTERS), false, 'no filters')
eq(hasActiveFilters({ ...DEFAULT_HUB_FILTERS, tab: 'draft' }), true, 'tab counts as a filter')
eq(hasActiveFilters({ ...DEFAULT_HUB_FILTERS, q: '  ' }), false, 'blank search is no filter')
eq(hasActiveFilters({ ...DEFAULT_HUB_FILTERS, sort: 'title', page: 3 }), false, 'sort and page are not filters')

console.log('content hub tests ok')

// --- the pager runs to the longest list, exams included -----------------------------------------------------------------
// 45 exams and 3 posts: the server lists are empty on page 2 but the exams still fill it, so page 2 is a real page
eq(clampPage(1, [3, 0, 0, 45], 20), 1, 'a page only the exams fill is kept')
eq(clampPage(2, [3, 0, 0, 45], 20), 2, 'the last exam page is kept')
eq(clampPage(3, [3, 0, 0, 45], 20), 2, 'beyond the last exam page: step back to it')
eq(clampPage(1, [3, 0, 0, 0], 20), 0, 'the last item of the last page was removed: back to the first page')
eq(clampPage(9, [0, 0, 0, 0], 20), 0, 'nothing at all: page one')
eq(clampPage(-4, [50, 0, 0, 0], 20), 0, 'a negative page')
eq(clampPage(2, [41, 0, 0], 20), 2, 'exactly on the last page of a server list')
eq(clampPage(3, [41, 0, 0], 20), 2, 'one past it')

// --- sorting the exams (the unpaged list) ---------------------------------------------------------------------------
const sortRows = [{ title: 'beta' }, { title: 'Alpha' }, { title: 'gamma' }]
eq(sortExams(sortRows, 'updated').map((e) => e.title), ['beta', 'Alpha', 'gamma'], 'updated keeps the server order')
eq(sortExams(sortRows, 'title', 'en').map((e) => e.title), ['Alpha', 'beta', 'gamma'], 'title is A-Z, case-insensitive')
eq(sortRows.map((e) => e.title), ['beta', 'Alpha', 'gamma'], 'the input is not reordered')
eq(sortExams([{ title: 'ب' }, { title: 'أ' }, { title: 'ت' }], 'title', 'ar').map((e) => e.title), ['أ', 'ب', 'ت'], 'Arabic order')
