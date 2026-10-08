import type { TeacherStats } from '@/api/platformTeacher'

// Pure helpers behind the teacher overview page: the numbers it shows, the "waiting for you" list, the quick-action
// gating and the first-run checklist. Kept free of Vue so they can be tested with a plain script.

const n = (v: number | undefined | null) => (typeof v === 'number' && Number.isFinite(v) ? v : 0)

/** Everything not published yet: article/summary and course drafts plus draft exams. */
export function draftsCount(s: TeacherStats): number {
  return n(s.content?.posts_draft) + n(s.content?.courses_draft) + n(s.assessments?.draft)
}

/** Everything students can see right now: published posts/courses, upcoming live sessions and published exams. */
export function publishedCount(s: TeacherStats): number {
  return n(s.content?.posts_published) + n(s.content?.courses_published) + n(s.content?.live_upcoming) + n(s.assessments?.published)
}

/** Exams in every state. */
export function examsCount(s: TeacherStats): number {
  return n(s.assessments?.draft) + n(s.assessments?.published) + n(s.assessments?.closed)
}

/** Subject requests of any outcome. */
export function requestedSubjects(s: TeacherStats): number {
  return n(s.teaching?.pending) + n(s.teaching?.approved) + n(s.teaching?.rejected)
}

export type TodoId = 'grading' | 'pending' | 'rejected' | 'unread' | 'drafts'
export interface TodoItem { id: TodoId; n: number; to: string }

/** Things that wait on the teacher, most urgent first; only rows with something to do are returned. */
export function todoItems(s: TeacherStats): TodoItem[] {
  const all: TodoItem[] = [
    { id: 'grading', n: n(s.pending_grading?.answers), to: '/platform/grading' },
    { id: 'pending', n: n(s.teaching?.pending), to: '/platform/teaching' },
    { id: 'rejected', n: n(s.teaching?.rejected), to: '/platform/teaching' },
    { id: 'unread', n: n(s.unread), to: '/platform/notifications' },
    { id: 'drafts', n: draftsCount(s), to: '/platform/my-content' },
  ]
  return all.filter((t) => t.n > 0)
}

export type QuickKind = 'subjects' | 'content' | 'post' | 'course' | 'live' | 'exam'
export type QuickBlock = 'no_subject' | 'exams_off' | null

/**
 * Why a quick action is unavailable, or null when it is. Creating anything needs an approved subject; exams also need the
 * admin's `can_create_exams` switch (only an explicit `false` turns it off). Without numbers (still loading or failed)
 * nothing is blocked client-side: the server decides.
 */
export function quickBlock(s: TeacherStats | null, kind: QuickKind): QuickBlock {
  if (!s || kind === 'subjects' || kind === 'content') return null
  if (n(s.teaching?.approved) === 0) return 'no_subject'
  if (kind === 'exam' && s.can_create_exams === false) return 'exams_off'
  return null
}

export type StepId = 'bio' | 'subject' | 'publish'
export interface ChecklistStep { id: StepId; done: boolean; to: string }

/**
 * First-run checklist. `hasBio` is null when the profile could not be read: that step is then left out instead of
 * claiming something unknown. The "publish" step points at the subjects page until there is an approved subject.
 */
export function onboardingSteps(s: TeacherStats | null, hasBio: boolean | null): ChecklistStep[] {
  if (!s) return []
  const steps: ChecklistStep[] = []
  if (hasBio !== null) steps.push({ id: 'bio', done: hasBio, to: '/platform/profile' })
  steps.push({ id: 'subject', done: requestedSubjects(s) > 0, to: '/platform/teaching' })
  steps.push({ id: 'publish', done: publishedCount(s) > 0, to: n(s.teaching?.approved) > 0 ? '/platform/my-content' : '/platform/teaching' })
  return steps
}

/** The checklist disappears once every step is done. */
export const checklistComplete = (steps: ChecklistStep[]): boolean => steps.length > 0 && steps.every((x) => x.done)
