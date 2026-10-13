import { platformBlob, platformFetch } from '@/lib/platformApi'
import { saveBlob } from '@/utils/saveFile'
import type { Phase } from './platformExamAdmin'

// The student's own screens (phase 4-1). Every figure is computed by the server from the session user; nothing here
// sends a student id or a role.

/** What one exam is for the signed-in student (one server rule, shared by "My exams", the overview and the exam page). */
export type StudentExamState = 'in_progress' | 'open' | 'upcoming' | 'done' | 'missed'
/** Why the exam cannot be started now (null when it can, or while an attempt is in progress). */
export type StudentExamReason = 'not_enrolled' | 'not_open' | 'closed' | 'no_attempts'

/** The student's best submitted attempt. score/percent/passed are null while the result is withheld (`released` false). */
export interface BestAttempt {
  attempt_id: string
  score: number | null
  total: number
  percent: number | null
  passed: boolean | null
  released: boolean
  /** written answers of this attempt still waiting for a grade (null while withheld) */
  pending?: number | null
}

export interface StudentExamRow {
  id: string
  title: string
  subject_id: string
  subject_name: string
  teacher_name: string
  question_count: number
  total_points: number
  duration_min: number | null
  opens_at: number | null
  closes_at: number | null
  max_attempts: number
  pass_mark: number | null
  release_mode: 'immediate' | 'after_close'
  /** the effective phase (a published exam past its closing time reads as closed) */
  phase: Phase
  state: StudentExamState
  can_start: boolean
  reason: StudentExamReason | null
  attempts_used: number
  in_progress_attempt: { attempt_id: string; ends_at: number } | null
  best: BestAttempt | null
  /** newest attempt of any status */
  last_attempt_id: string | null
  /** false: the exam is no longer shown to students (hidden subject, archived, unenrolled); the row only leads to my own results */
  visible: boolean
  /**
   * a running attempt the student can still go back into: they are enrolled in the subject, even when the exam was hidden
   * meanwhile (the server lets an enrolled student back into a sitting in the middle). Absent from an older server: `visible` then decides.
   */
  can_resume?: boolean
}

export type StudentExamTab = 'all' | StudentExamState
export type StudentExamCounts = Record<StudentExamState, number>
export interface StudentExamList { items: StudentExamRow[]; total: number; counts: StudentExamCounts }
export interface StudentExamQuery { tab?: StudentExamTab; subject_id?: string; q?: string; limit?: number; offset?: number }

export function myExams(f: StudentExamQuery = {}): Promise<StudentExamList> {
  const p = new URLSearchParams()
  if (f.tab && f.tab !== 'all') p.set('tab', f.tab)
  if (f.subject_id) p.set('subject_id', f.subject_id)
  if (f.q) p.set('q', f.q)
  if (f.limit !== undefined) p.set('limit', String(f.limit))
  if (f.offset) p.set('offset', String(f.offset))
  const qs = p.toString()
  return platformFetch(`/my/exams${qs ? `?${qs}` : ''}`)
}

export interface StudentOverview {
  enrolled: number
  unread: number
  /** an attempt still running: the overview's first card */
  resume: { attempt_id: string; assessment_id: string; title: string; ends_at: number } | null
  exams: {
    open: number
    upcoming: number
    in_progress: number
    /** startable now and closing within 48 hours, soonest first (at most 5) */
    closing_soon: { id: string; title: string; subject_name: string; closes_at: number }[]
  }
  /** my submitted attempts that still wait for a teacher's grade */
  awaiting_grading: number
  /** newest 3 submitted attempts; score/percent/passed are null while withheld */
  recent_results: {
    attempt_id: string; assessment_id: string; title: string
    score: number | null; total: number; percent: number | null; passed: boolean | null; released: boolean; submitted_at: number
    /** written answers still waiting for a grade (null while withheld) */
    pending?: number | null
  }[]
  /** started and not finished, newest activity first (at most 4) */
  courses: { course_id: string; title: string; subject_name: string; percent: number; next_lesson_id: string | null }[]
  next_live: { id: string; title: string; subject_id: string; subject_name: string; starts_at: number; duration_min: number } | null
  checklist: { placement: boolean; enrolled: boolean }
}

export const studentOverview = () => platformFetch<StudentOverview>('/student/overview')

export interface SuggestedSubject {
  id: string
  name_ar: string
  name_en: string | null
  /** unit names from the institution's root down to the subject's unit */
  unit_path: string[]
  enrolled: boolean
}
export interface SuggestedSubjects {
  placement: { institution_id: string; institution_name: string; unit_id: string | null; unit_path: string[] } | null
  subjects: SuggestedSubject[]
}

/** The active subjects of my level (and the levels below it) in the institution I placed myself in. */
export const suggestedSubjects = () => platformFetch<SuggestedSubjects>('/me/suggested-subjects')

/** At most this many subjects per bulk enrolment request (the server answers 400 `invalid_selection` otherwise). */
export const BULK_ENROLL_MAX = 30
/** Each id goes through the single-enrolment rules; one that fails a rule or is already enrolled is counted as skipped. */
export const bulkEnroll = (subject_ids: string[]) =>
  platformFetch<{ enrolled: number; skipped: number }>('/enrollments/bulk', { body: { subject_ids } })

// ── phase 4-2: learning hub, schedule, live page, following and saved items ──────────────────────────────────────────

export type LearningState = 'in_progress' | 'not_started' | 'completed'
export type LearningTab = 'all' | LearningState
export interface LearningItem {
  course_id: string
  title: string
  /** the first 160 characters of the description */
  description: string | null
  subject_id: string
  subject_name: string
  teacher_id: string
  teacher_name: string
  total: number
  completed: number
  /** 0..100, two decimals */
  percent: number
  state: LearningState
  /** the first lesson not completed yet (null once the course is completed) */
  next_lesson_id: string | null
  /** when I last completed a lesson of this course */
  last_at: number | null
  updated_at: number
}
/** Counts after the subject / search filters, whatever the state filter is. */
export type LearningCounts = Record<LearningState, number>
export interface LearningList { items: LearningItem[]; total: number; counts: LearningCounts }
export interface LearningQuery { state?: LearningTab; subject_id?: string; q?: string; limit?: number; offset?: number }

export function myLearning(f: LearningQuery = {}): Promise<LearningList> {
  const p = new URLSearchParams()
  if (f.state && f.state !== 'all') p.set('state', f.state)
  if (f.subject_id) p.set('subject_id', f.subject_id)
  if (f.q) p.set('q', f.q)
  if (f.limit !== undefined) p.set('limit', String(f.limit))
  if (f.offset) p.set('offset', String(f.offset))
  const qs = p.toString()
  return platformFetch(`/my/learning${qs ? `?${qs}` : ''}`)
}

export type ScheduleKind = 'live' | 'exam_opens' | 'exam_closes'
/** One entry of my timetable. The join link is never part of it: the live page decides what I may see. */
export interface ScheduleItem {
  kind: ScheduleKind
  /** the live session's or the exam's id */
  id: string
  title: string
  subject_id: string
  subject_name: string
  teacher_name: string
  /** live: the start; exams: the opening / closing time */
  at: number
  /** live only: start + duration */
  ends_at: number | null
  /** live only */
  duration_min: number | null
  /** live only: the join link is withheld (not enrolled in the subject) */
  locked: boolean
  /** an in-app path: /platform/live/<id> or /platform/assessments/<id> */
  link: string
}
/** `from` / `to` are epoch ms; the server defaults to now - 2 h .. + 7 days and refuses more than 62 days (400 `invalid_range`). */
export const mySchedule = (from: number, to: number) =>
  platformFetch<{ items: ScheduleItem[] }>(`/my/schedule?from=${Math.round(from)}&to=${Math.round(to)}`)

export type LivePhase = 'upcoming' | 'live_now' | 'ended'
/** `GET /live/{id}` for a student (the owner and admins get the editor's shape from `getLive`). */
export interface StudentLive {
  id: string
  title: string
  description: string | null
  teacher_id: string
  teacher_name: string
  subject_id: string
  subject_name: string
  starts_at: number
  duration_min: number
  status: 'scheduled' | 'cancelled'
  /** "" while locked */
  join_url: string
  locked: boolean
  /** I am enrolled in the subject */
  enrolled: boolean
  phase: LivePhase
  /** starts_at - 15 minutes */
  joinable_from: number
}
export const studentLive = (id: string) => platformFetch<StudentLive>(`/live/${encodeURIComponent(id)}`)

export interface FollowedTeacher { id: string; full_name: string; bio: string | null; subject_count: number; followed_at: number }
export const myFollowing = () => platformFetch<{ items: FollowedTeacher[] }>('/my/following')

export type BookmarkType = 'post' | 'course'
export interface BookmarkRow {
  target_type: BookmarkType
  target_id: string
  title: string
  subject_name: string
  teacher_name: string
  saved_at: number
  updated_at: number
}
/** At most this many saved items per student (the server answers 409 `too_many_bookmarks` beyond it). */
export const BOOKMARK_MAX = 200
export const myBookmarks = () => platformFetch<{ items: BookmarkRow[] }>('/my/bookmarks')
export const putBookmark = (target_type: BookmarkType, target_id: string) =>
  platformFetch<void>('/bookmarks', { method: 'PUT', body: { target_type, target_id } })
export const deleteBookmark = (target_type: BookmarkType, target_id: string) =>
  platformFetch<void>(`/bookmarks/${target_type}/${encodeURIComponent(target_id)}`, { method: 'DELETE' })

/**
 * Downloads my timetable (live sessions and exam windows, from yesterday to 90 days ahead) as a calendar file. `lang` is the
 * language of the file's fixed words ("Teacher", "Exam opens", ...): the interface language, so the file reads like the page.
 */
export async function downloadScheduleIcs(lang: 'ar' | 'en' = 'ar'): Promise<void> {
  const blob = await platformBlob(`/my/schedule.ics?lang=${lang}`)
  saveBlob(new Blob([blob], { type: 'text/calendar;charset=utf-8' }), 'azlearn-schedule.ics')
}

// ── phase 4-3: my own statistics (private: no ranking, no other student's data) ───────────────────────────────────────────
export interface StatsTrendPoint { attempt_id: string; exam_id: string; exam_title: string; percent: number; submitted_at: number }
export interface WeakChapter { chapter: string; percent: number; answered: number }
export interface SubjectStats {
  subject_id: string
  subject_name: string
  exams: number
  attempts: number
  /** average of my best attempt per exam; null without any counted attempt */
  average_percent: number | null
  /** the last 8 counted attempts of this subject, oldest first */
  trend: StatsTrendPoint[]
  /** chapters with at least 3 answered items under 70 %, lowest first, at most 3 */
  weakest: WeakChapter[]
}
export interface StudentStats {
  overall: { attempts: number; exams: number; average_percent: number | null }
  subjects: SubjectStats[]
}
/** Only my submitted, released, fully graded attempts count. */
export const myStats = () => platformFetch<StudentStats>('/my/stats')
