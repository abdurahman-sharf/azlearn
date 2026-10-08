import { allowedKinds, ancestors, buildIndex, children, descendants, pathToSubject, rollup, subtreeSubjects, type SubjectNumbers, type TreeSubject, type TreeUnit } from './structureTree.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const u = (id: string, parent: string | null, kind: string): TreeUnit => ({ id, parent_id: parent, kind, name_ar: id, is_active: true })
const s = (id: string, unit: string | null): TreeSubject => ({ id, unit_id: unit, is_active: true })
const ids = (xs: { id: string }[]) => xs.map((x) => x.id)

// commerce › level1 › year2025 › term1, commerce › level2, law (root), plus subjects everywhere
const units = [u('commerce', null, 'department'), u('l1', 'commerce', 'level'), u('y25', 'l1', 'year'), u('t1', 'y25', 'term'), u('l2', 'commerce', 'level'), u('law', null, 'department')]
const subjects = [s('acc', 't1'), s('math', 'l1'), s('eco', 'l2'), s('civil', 'law'), s('loose', null)]
const idx = buildIndex(units, subjects)

eq(ids(children(idx, null)), ['commerce', 'law'], 'roots in given order')
eq(ids(children(idx, 'commerce')), ['l1', 'l2'], 'children')
eq(ids(children(idx, 't1')), [], 'a leaf has no children')
eq(ids(descendants(idx, 'commerce')), ['l1', 'y25', 't1', 'l2'], 'descendants, parents first')
eq(ids(descendants(idx, null)).length, 6, 'null = every unit')

// subjects of a subtree: the node itself, below it, and `null` = everything
eq([...ids(subtreeSubjects(idx, 'commerce'))].sort(), ['acc', 'eco', 'math'], 'commerce subtree members')
eq(ids(subtreeSubjects(idx, 'l1')).sort(), ['acc', 'math'], 'a subject on a non-leaf unit is included with those below')
eq(ids(subtreeSubjects(idx, 't1')), ['acc'], 'leaf')
eq(ids(subtreeSubjects(idx, null)), ['acc', 'math', 'eco', 'civil', 'loose'], 'null = all subjects, including the one on the institution')
eq(ids(idx.subjectsAt.get(null) ?? []), ['loose'], 'a subject without a unit hangs on the institution')

eq(ids(ancestors(idx, 't1')), ['commerce', 'l1', 'y25'], 'ancestors root first, excluding self')
eq(ids(ancestors(idx, 'commerce')), [], 'a root has none')
eq(ids(ancestors(idx, 'nope')), [], 'unknown id')
eq(ids(pathToSubject(idx, 'acc')), ['commerce', 'l1', 'y25', 't1'], 'path to a subject includes its own unit')
eq(ids(pathToSubject(idx, 'loose')), [], 'institution-level subject has no path')
eq(ids(pathToSubject(idx, 'ghost')), [], 'unknown subject')

// roll-up numbers
const numbers = new Map<string, SubjectNumbers>([
  ['acc', { exams: 2, attempts: 10, questions: 30 }],
  ['math', { exams: 1, attempts: 4, questions: 12 }],
  ['civil', { exams: 5, attempts: 50, questions: 100 }],
])
eq(rollup(idx, 'commerce', numbers), { childUnits: 2, descendantUnits: 4, subjects: 3, exams: 3, attempts: 14, questions: 42 }, 'commerce roll-up (eco has no numbers → 0)')
eq(rollup(idx, null, numbers), { childUnits: 2, descendantUnits: 6, subjects: 5, exams: 8, attempts: 64, questions: 142 }, 'institution roll-up')
eq(rollup(idx, 't1'), { childUnits: 0, descendantUnits: 0, subjects: 1, exams: 0, attempts: 0, questions: 0 }, 'no numbers given → zeros')

// defensive: orphans become roots; cycles and self-parents do not loop forever
const odd = buildIndex([u('a', 'ghost', 'level'), u('b', 'c', 'year'), u('c', 'b', 'year'), u('d', 'd', 'term')], [s('x', 'ghost'), s('y', 'b')])
eq(ids(children(odd, null)).sort(), ['a', 'd'], 'orphan and self-parent are roots')
ok(descendants(odd, 'b').length <= 2, 'a cycle terminates')
ok(ancestors(odd, 'b').length <= 2, 'ancestors of a cycle terminate')
eq(ids(odd.subjectsAt.get(null) ?? []), ['x'], 'a subject of a missing unit hangs on the institution')

// which unit kinds can be created where (mirrors the server rules)
eq(allowedKinds(null, false), ['department', 'level', 'year', 'term'], 'top level of a university')
eq(allowedKinds(null, true), ['level', 'year', 'term'], 'schools have no departments')
eq(allowedKinds('level', false), ['year', 'term'], 'only deeper kinds under a level')
eq(allowedKinds('term', false), [], 'nothing under a term')
eq(allowedKinds('department', true), ['level', 'year', 'term'], 'a school never offers department')

console.log('structure tree tests ok')
