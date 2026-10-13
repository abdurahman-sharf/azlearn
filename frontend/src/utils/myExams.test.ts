import { clampPage, filtersFromQuery, filtersToQuery, listFilter, rowActions, DEFAULT_MY_FILTERS, type ActionRow } from './myExams.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- the list request of each tab -------------------------------------------------------------------------------------
eq(listFilter(DEFAULT_MY_FILTERS), { limit: 25, offset: 0 }, 'all: no status, so the server hides archived ones')
eq(listFilter({ ...DEFAULT_MY_FILTERS, tab: 'draft' }), { limit: 25, offset: 0, status: 'draft' }, 'draft tab')
eq(listFilter({ ...DEFAULT_MY_FILTERS, tab: 'archived' }).status, 'archived', 'archived tab')
eq(listFilter({ ...DEFAULT_MY_FILTERS, tab: 'admin' }), { limit: 25, offset: 0, scope: 'admin' }, 'admin tab: scope admin, no status')
eq(listFilter({ tab: 'closed', q: '  mid  ', subjectId: 's1', page: 2 }), { limit: 25, offset: 50, status: 'closed', q: 'mid', subject_id: 's1' }, 'search, subject and page')
eq(listFilter({ ...DEFAULT_MY_FILTERS, page: -3 }).offset, 0, 'a negative page is the first page')

// --- the address bar ----------------------------------------------------------------------------------------------------
eq(filtersToQuery(DEFAULT_MY_FILTERS), {}, 'defaults leave the address clean')
eq(filtersToQuery({ tab: 'admin', q: ' x ', subjectId: 's9', page: 1 }), { q: 'x', tab: 'admin', subject: 's9', page: '2' }, 'only what differs, page one-based')
eq(filtersFromQuery({ tab: 'published', q: 'abc', subject: 's1', page: '3' }), { tab: 'published', q: 'abc', subjectId: 's1', page: 2 }, 'read back')
eq(filtersFromQuery({ tab: 'bogus', page: '-4' }), DEFAULT_MY_FILTERS, 'junk falls back to the defaults')
eq(filtersFromQuery({ tab: ['draft', 'closed'], page: ['2'] }), { ...DEFAULT_MY_FILTERS, tab: 'draft', page: 1 }, 'repeated keys use the first')
eq(filtersFromQuery(filtersToQuery({ tab: 'closed', q: 'a', subjectId: 'z', page: 4 })), { tab: 'closed', q: 'a', subjectId: 'z', page: 4 }, 'round trip')

// --- paging -------------------------------------------------------------------------------------------------------------
eq(clampPage(3, 51), 2, 'the last page that exists (51 rows = 3 pages)')
eq(clampPage(1, 25), 0, 'exactly one page')
eq(clampPage(0, 0), 0, 'nothing at all')
eq(clampPage(-2, 80), 0, 'negative')

// --- row actions --------------------------------------------------------------------------------------------------------
const row = (p: Partial<ActionRow>): ActionRow => ({ status: 'draft', can_edit: true, locked: false, attempt_count: 0, pending_answers: 0, ...p })
const on = { examsOff: false, readOnly: false }
const off = { examsOff: true, readOnly: false }

eq(rowActions(row({}), on).shown, ['edit', 'publish', 'duplicate', 'archive', 'delete'], 'draft')
eq(rowActions(row({ status: 'published' }), on).shown, ['edit', 'unpublish', 'close', 'duplicate', 'delete'], 'published, no attempts')
eq(rowActions(row({ status: 'published', attempt_count: 4, pending_answers: 2 }), on),
  { shown: ['edit', 'close', 'duplicate', 'results', 'grading'], disabled: [], deleteBlocked: true }, 'published with attempts: no unpublish, no delete, explanation instead')
eq(rowActions(row({ status: 'closed', attempt_count: 1 }), on).shown, ['edit', 'reopen', 'duplicate', 'archive', 'results'], 'closed with attempts')
eq(rowActions(row({ status: 'closed' }), on).shown, ['edit', 'reopen', 'duplicate', 'archive', 'delete'], 'closed')
eq(rowActions(row({ status: 'archived', can_edit: false }), on).shown, ['restore', 'duplicate', 'delete'], 'archived: restore, no edit')
eq(rowActions(row({ status: 'archived', can_edit: false, attempt_count: 2 }), on).deleteBlocked, true, 'archived with attempts cannot be deleted')

// locked by an admin: only a copy and the results
eq(rowActions(row({ status: 'closed', can_edit: false, locked: true, attempt_count: 3, pending_answers: 1 }), on),
  { shown: ['duplicate', 'results', 'grading'], disabled: [], deleteBlocked: false }, 'locked: copy, results, grading')
eq(rowActions(row({ status: 'archived', can_edit: false, locked: true }), on).shown, ['duplicate', 'delete'], 'locked and archived, nobody sat it: copy, and delete (the server allows it)')
eq(rowActions(row({ status: 'closed', can_edit: false, locked: true, attempt_count: 1 }), on).shown.includes('delete'), false, 'locked with attempts: never delete')

// the admin's exams: read-only
eq(rowActions(row({ status: 'published', can_edit: false, attempt_count: 5, pending_answers: 3 }), { examsOff: false, readOnly: true }),
  { shown: ['results', 'grading'], disabled: [], deleteBlocked: false }, 'admin exam: results and grading')
eq(rowActions(row({ status: 'published', can_edit: false }), { examsOff: false, readOnly: true }).shown, [], 'admin exam without attempts: nothing')

// the switch: publish and copy are disabled, close/reopen/archive/results stay
const offDraft = rowActions(row({}), off)
eq(offDraft.shown, ['edit', 'publish', 'duplicate', 'archive', 'delete'], 'switch off keeps the actions listed')
eq(offDraft.disabled, ['publish', 'duplicate'], 'but publish and copy are disabled')
eq(rowActions(row({ status: 'closed' }), off).disabled, ['duplicate'], 'reopen does not need the switch')
eq(rowActions(row({ status: 'published' }), off).disabled, ['duplicate'], 'close does not need the switch')
eq(rowActions(row({ status: 'closed', locked: true, can_edit: false }), off).disabled, ['duplicate'], 'a locked exam can only be copied, and copying needs the switch')

console.log('my exams tests ok')
