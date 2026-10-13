import type { TeacherStats } from '@/api/platformTeacher'
import { checklistComplete, draftsCount, examsCount, onboardingSteps, publishedCount, quickBlock, requestedSubjects, todoItems } from './teacherOnboarding.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const base = (): TeacherStats => ({
  teaching: { pending: 0, approved: 0, rejected: 0 },
  content: { posts_draft: 0, posts_published: 0, courses_draft: 0, courses_published: 0, live_upcoming: 0 },
  assessments: { draft: 0, published: 0, closed: 0, attempts_submitted: 0 },
  pending_grading: { answers: 0, exams: 0 },
  unread: 0,
  can_create_exams: true,
})
const withStats = (f: (s: TeacherStats) => void): TeacherStats => {
  const s = base()
  f(s)
  return s
}

// --- counters ---------------------------------------------------------------------------------------------------
const busy = withStats((s) => {
  s.teaching = { pending: 1, approved: 2, rejected: 3 }
  s.content = { posts_draft: 4, posts_published: 5, courses_draft: 6, courses_published: 7, live_upcoming: 8 }
  s.assessments = { draft: 9, published: 10, closed: 11, attempts_submitted: 12 }
})
eq(draftsCount(busy), 4 + 6 + 9, 'drafts = post + course + exam drafts')
eq(publishedCount(busy), 5 + 7 + 8 + 10, 'published = posts + courses + upcoming live + published exams')
eq(examsCount(busy), 9 + 10 + 11, 'exams of every state, submissions are not exams')
eq(requestedSubjects(busy), 6, 'requests of every outcome')
eq([draftsCount(base()), publishedCount(base()), examsCount(base()), requestedSubjects(base())], [0, 0, 0, 0], 'a new teacher has zeros')
// a malformed answer never produces NaN
const broken = { teaching: {}, content: null, assessments: undefined } as unknown as TeacherStats
eq([draftsCount(broken), publishedCount(broken), examsCount(broken), requestedSubjects(broken)], [0, 0, 0, 0], 'missing parts count as zero')

// --- waiting-for-you list -----------------------------------------------------------------------------------------
eq(todoItems(base()), [], 'nothing waiting')
const waiting = todoItems(withStats((s) => {
  s.pending_grading = { answers: 7, exams: 2 }
  s.teaching.pending = 1
  s.teaching.rejected = 2
  s.unread = 3
  s.assessments.draft = 1
}))
eq(waiting.map((t) => t.id), ['grading', 'pending', 'rejected', 'unread', 'drafts'], 'urgent first')
eq(waiting.map((t) => t.n), [7, 1, 2, 3, 1], 'counters')
eq(waiting.map((t) => t.to), ['/platform/grading', '/platform/teaching', '/platform/teaching', '/platform/notifications', '/platform/my-content'], 'each row links to the screen that resolves it')
eq(todoItems(withStats((s) => { s.unread = 2 })).map((t) => t.id), ['unread'], 'only rows with something to do')

// --- quick actions ------------------------------------------------------------------------------------------------
eq(quickBlock(null, 'exam'), null, 'unknown numbers block nothing (the server decides)')
eq(quickBlock(base(), 'subjects'), null, 'requesting subjects is always possible')
eq(quickBlock(base(), 'content'), null, 'the content hub is always reachable')
for (const k of ['post', 'course', 'live', 'exam'] as const) eq(quickBlock(base(), k), 'no_subject', `${k} needs an approved subject`)
const approved = withStats((s) => { s.teaching.approved = 1 })
for (const k of ['post', 'course', 'live', 'exam'] as const) eq(quickBlock(approved, k), null, `${k} allowed with an approved subject`)
const examsOff = withStats((s) => { s.teaching.approved = 1; s.can_create_exams = false })
eq(quickBlock(examsOff, 'exam'), 'exams_off', 'the admin switch stops exams')
eq(quickBlock(examsOff, 'post'), null, 'the admin switch does not stop other content')
eq(quickBlock(withStats((s) => { s.can_create_exams = false }), 'exam'), 'no_subject', 'no subject is reported first')
// only an explicit false switches exams off: a missing flag (older server) must not lock anyone out
const noFlag = approved as unknown as Record<string, unknown>
delete noFlag.can_create_exams
eq(quickBlock(noFlag as unknown as TeacherStats, 'exam'), null, 'a missing flag means enabled')

// --- first-run checklist ------------------------------------------------------------------------------------------
eq(onboardingSteps(null, true), [], 'no numbers, no checklist')
const fresh = onboardingSteps(base(), false)
eq(fresh.map((x) => x.id), ['bio', 'subject', 'publish'], 'three steps in order')
eq(fresh.map((x) => x.done), [false, false, false], 'a new teacher has done nothing')
eq(fresh.map((x) => x.to), ['/platform/profile', '/platform/teaching', '/platform/teaching'], 'publishing starts at subjects until one is approved')
ok(!checklistComplete(fresh), 'not complete')
eq(onboardingSteps(base(), null).map((x) => x.id), ['subject', 'publish'], 'an unreadable profile leaves the bio step out')
const requested = onboardingSteps(withStats((s) => { s.teaching.pending = 1 }), true)
eq(requested.map((x) => x.done), [true, true, false], 'a pending request counts as requested')
const approvedSteps = onboardingSteps(withStats((s) => { s.teaching.approved = 1 }), false)
eq(approvedSteps.find((x) => x.id === 'publish')?.to, '/platform/my-content', 'publishing points at the hub once a subject is approved')
const done = onboardingSteps(withStats((s) => { s.teaching.approved = 1; s.content.posts_published = 1 }), true)
ok(checklistComplete(done), 'complete when all three are done')
ok(!checklistComplete([]), 'an empty list is never "complete"')
ok(checklistComplete(onboardingSteps(withStats((s) => { s.teaching.approved = 1; s.assessments.published = 1 }), null)), 'complete without the unknown bio step')

console.log('teacher onboarding tests ok')
