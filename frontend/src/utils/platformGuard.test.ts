import { isExamTakePath, pendingTeacherMayEnter } from './platformGuard.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (a !== b) throw new Error(`${msg}: expected ${String(b)}, got ${String(a)}`)
}

// --- who may use a pendingTeacherOk route ---------------------------------------------------------------------------
const flagged = { pendingTeacherOk: true }
const matrix: Array<[string | null, string | null, boolean]> = [
  ['teacher', 'pending', true], // the point: a waiting teacher can request subjects
  ['teacher', 'active', false], // active accounts do not need the exception (they pass the normal check)
  ['teacher', 'rejected', false],
  ['teacher', 'suspended', false],
  ['student', 'pending', false],
  ['admin', 'pending', false],
  [null, 'pending', false],
  ['teacher', null, false],
]
for (const [role, status, want] of matrix) eq(pendingTeacherMayEnter(flagged, role, status), want, `flagged route ${role}/${status}`)
// a route without the flag never lets a pending teacher in, whatever the role/status
for (const [role, status] of matrix) eq(pendingTeacherMayEnter({}, role, status), false, `unflagged route ${role}/${status}`)
eq(pendingTeacherMayEnter({ pendingTeacherOk: 'yes' }, 'teacher', 'pending'), false, 'only a real true opts in')

// --- the exam page is recognised, other pages are not -----------------------------------------------------------------
eq(isExamTakePath('/platform/assessments/abc123/take'), true, 'take page')
eq(isExamTakePath('/platform/assessments/abc123/take?x=1'), true, 'take page with query')
eq(isExamTakePath('/platform/assessments/abc123'), false, 'exam info page')
eq(isExamTakePath('/platform/assessments/abc123/results'), false, 'results page')
eq(isExamTakePath('/platform/assessments//take'), false, 'empty id')
eq(isExamTakePath('/platform/assessments/a/b/take'), false, 'nested id')
eq(isExamTakePath('/other/platform/assessments/abc/take'), false, 'prefix')
eq(isExamTakePath(undefined), false, 'not a string')
eq(isExamTakePath(['/platform/assessments/abc/take']), false, 'array query value')

console.log('platform guard tests ok')
