import { groupByStatus, impactTotal, isUsable, placeLabel, statusBySubject, usableSubjects } from './teachingCards.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

type Row = Parameters<typeof groupByStatus>[0][number] & { subject_id: string; subject_active: boolean; institution_active: boolean }
const row = (subject_id: string, status: Row['status'], created_at: number, decided_at: number | null = null, extra: Partial<Row> = {}): Row => ({
  subject_id, subject_name: subject_id, status, created_at, decided_at, subject_active: true, institution_active: true, ...extra,
})

// --- grouping ----------------------------------------------------------------------------------------------------
const rows = [
  row('p2', 'pending', 200), row('r1', 'rejected', 10, 50), row('a1', 'approved', 10, 100), row('p1', 'pending', 100),
  row('a2', 'approved', 20, 300), row('r2', 'rejected', 10, 400),
]
const groups = groupByStatus(rows)
eq(groups.map((g) => g.status), ['approved', 'pending', 'rejected'], 'groups in display order')
eq(groups[0]!.rows.map((r) => r.subject_id), ['a2', 'a1'], 'approved: latest decision first')
eq(groups[1]!.rows.map((r) => r.subject_id), ['p1', 'p2'], 'pending: oldest request first (queue order)')
eq(groups[2]!.rows.map((r) => r.subject_id), ['r2', 'r1'], 'rejected: latest decision first')
eq(groupByStatus([row('p1', 'pending', 1)]).map((g) => g.status), ['pending'], 'empty groups are left out')
eq(groupByStatus([]), [], 'nothing at all')
// the input array is not reordered
eq(rows.map((r) => r.subject_id), ['p2', 'r1', 'a1', 'p1', 'a2', 'r2'], 'input untouched')
// ties fall back to the subject name so the order is stable
eq(groupByStatus([row('b', 'approved', 1, 5), row('a', 'approved', 1, 5)])[0]!.rows.map((r) => r.subject_id), ['a', 'b'], 'tie broken by name')

// --- picker status map ---------------------------------------------------------------------------------------------
eq(statusBySubject(rows), { p2: 'pending', r1: 'rejected', a1: 'approved', p1: 'pending', a2: 'approved', r2: 'rejected' }, 'status by subject')
eq(statusBySubject([]), {}, 'empty map')

// --- what the editors may offer --------------------------------------------------------------------------------------
eq(isUsable(row('x', 'approved', 1)), true, 'approved and active')
eq(isUsable(row('x', 'approved', 1, null, { subject_active: false })), false, 'subject switched off')
eq(isUsable(row('x', 'approved', 1, null, { institution_active: false })), false, 'institution switched off')
eq(isUsable(row('x', 'pending', 1)), false, 'pending')
eq(isUsable(row('x', 'rejected', 1)), false, 'rejected')
eq(usableSubjects([row('a', 'approved', 1), row('b', 'approved', 1, null, { subject_active: false }), row('c', 'pending', 1)]).map((r) => r.subject_id), ['a'], 'only usable rows')
// an older server that sends no activity flags must not hide every subject
const legacy = { subject_id: 'l', subject_name: 'l', status: 'approved' as const, created_at: 1, decided_at: null } as unknown as Row
eq(isUsable(legacy), true, 'missing flags count as active')

// --- labels ---------------------------------------------------------------------------------------------------------
eq(placeLabel('Uni', ['Commerce', 'Level 1']), 'Uni › Commerce › Level 1', 'place with units')
eq(placeLabel('Uni', []), 'Uni', 'place on the institution')
eq(placeLabel('Uni', undefined), 'Uni', 'place without a path field')
eq(impactTotal({ posts: 1, courses: 2, live: 3, exams: 4 }), 10, 'impact total')
eq(impactTotal(null), 0, 'no impact known')

console.log('teaching cards tests ok')
