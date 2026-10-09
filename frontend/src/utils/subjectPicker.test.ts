import { activeSubset, restrictToSubjects, subjectSelectable } from './subjectPicker.ts'
import type { TreeSubject, TreeUnit } from './structureTree.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// --- which subjects can still be requested --------------------------------------------------------------------------
eq(subjectSelectable(undefined), true, 'no assignment: free')
eq(subjectSelectable('rejected'), true, 'rejected: can be asked again')
eq(subjectSelectable('pending'), false, 'pending: already asked')
eq(subjectSelectable('approved'), false, 'approved: already teaching')

// --- activeOnly ---------------------------------------------------------------------------------------------------
const u = (id: string, parent: string | null, active = true): TreeUnit => ({ id, parent_id: parent, kind: 'level', name_ar: id, is_active: active })
const s = (id: string, unit: string | null, active = true): TreeSubject => ({ id, unit_id: unit, is_active: active })
const ids = (xs: { id: string }[]) => xs.map((x) => x.id)

// commerce(active) > l1(active) > y25(INACTIVE) > t1(active); commerce > l2(active); law(INACTIVE) > l3(active)
const units = [u('commerce', null), u('l1', 'commerce'), u('y25', 'l1', false), u('t1', 'y25'), u('l2', 'commerce'), u('law', null, false), u('l3', 'law')]
const subjects = [s('a', 'l1'), s('b', 'y25'), s('c', 't1'), s('d', 'l2'), s('e', 'l3'), s('f', null), s('g', 'l2', false), s('h', 'ghost')]
const r = activeSubset(units, subjects)

eq(ids(r.units), ['commerce', 'l1', 'l2'], 'an inactive unit takes everything below it with it')
eq(ids(r.subjects), ['a', 'd', 'f', 'h'], 'subjects under an inactive unit, and inactive subjects, are dropped; a subject on the institution or with an unknown unit stays')

// everything active: nothing changes, order kept
const all = activeSubset([u('x', null), u('y', 'x')], [s('p', 'y'), s('q', null)])
eq(ids(all.units), ['x', 'y'], 'all active: units kept')
eq(ids(all.subjects), ['p', 'q'], 'all active: subjects kept')

// nothing at all
eq(activeSubset([], []), { units: [], subjects: [] }, 'empty input')

// a looping parent chain (bad data) must terminate
const loop = activeSubset([u('p', 'q'), u('q', 'p')], [s('z', 'p')])
eq(ids(loop.units), ['p', 'q'], 'a loop of active units ends without hanging')
eq(ids(loop.subjects), ['z'], 'subject in a loop of active units is kept')
const loopOff = activeSubset([u('p', 'q'), u('q', 'p', false)], [s('z', 'p')])
eq(ids(loopOff.units), [], 'a loop containing an inactive unit drops all of it')
eq(ids(loopOff.subjects), [], 'and the subjects under it')

// a unit whose parent is missing counts as a root
eq(ids(activeSubset([u('orphan', 'gone')], []).units), ['orphan'], 'orphan unit counts as a root')

// --- restrictToSubjects (a teacher may only pick the subjects they are approved for) -----------------------------------
const ru = [u('commerce', null), u('l1', 'commerce'), u('y25', 'l1'), u('t1', 'y25'), u('l2', 'commerce'), u('law', null), u('l3', 'law')]
const rs = [s('a', 'l1'), s('c', 't1'), s('d', 'l2'), s('e', 'l3'), s('f', null), s('h', 'ghost')]
const only = restrictToSubjects(ru, rs, new Set(['c', 'f']))
eq(ids(only.subjects), ['c', 'f'], 'only the allowed subjects remain, in order')
eq(ids(only.units), ['commerce', 'l1', 'y25', 't1'], 'the units on the way to an allowed subject stay (ancestors included), the rest go')
const two = restrictToSubjects(ru, rs, new Set(['d', 'e']))
eq(ids(two.units), ['commerce', 'l2', 'law', 'l3'], 'two branches: both ancestor chains stay, siblings without allowed subjects go')
eq(restrictToSubjects(ru, rs, new Set()), { units: [], subjects: [] }, 'nothing allowed: nothing left')
eq(ids(restrictToSubjects(ru, rs, new Set(['zzz'])).subjects), [], 'an allowed id that does not exist changes nothing')
// a subject whose unit is unknown (orphan) or on the institution itself needs no unit
const orphan = restrictToSubjects(ru, rs, new Set(['h', 'f']))
eq(ids(orphan.subjects), ['f', 'h'], 'institution-level and orphan subjects are kept')
eq(ids(orphan.units), [], 'and they keep no unit')
// a loop of units ends the walk
const loopR = restrictToSubjects([u('p', 'q'), u('q', 'p')], [s('z', 'p')], new Set(['z']))
eq(ids(loopR.units), ['p', 'q'], 'a loop terminates and keeps both units')

console.log('subject picker tests ok')
