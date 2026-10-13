// Whether the practice page (the exam's questions WITH their answers) can be opened by a student right now. The server decides
// for real (`platform_practice::practice_exam_at`); this mirrors its rules so the exam page offers the link, the "later" hint
// or nothing, instead of a link that ends in a refusal:
//  * the owner must have switched practice on, and the student must be enrolled in the subject;
//  * not while the student has a graded attempt running (a second tab must not read the key of the sitting it is in);
//  * not before the exam opens;
//  * not while the results are withheld (release mode `after_close`) and not yet released: the exam is closed, or its closing
//    time and the grace after it have passed (nobody can still hand in).

/** The grace after an attempt's end during which a last save or submit may still land (the server's `GRACE_MS`). */
export const PRACTICE_GRACE_MS = 60_000

/** `available`: show the link. `later`: practice exists but not yet - say when. `hidden`: say nothing about it. */
export type PracticeAccess = 'available' | 'later' | 'hidden'

export interface PracticeFacts {
  allowPractice: boolean | null | undefined
  /** the exam page's reason for not starting (`not_enrolled` means the student is outside the subject) */
  reason?: string | null
  /** the student's state of the exam (`in_progress` = a graded attempt is running) */
  state?: string | null
  /** the exam's lifecycle status (`published`, `closed`...) */
  status: string
  opensAt: number | null
  releaseMode: string
  closesAt: number | null
  /** the server's clock (epoch ms) */
  now: number
}

/** The server's `is_released`: results and answers are shown at once, or once nobody can still hand in an attempt. */
export function resultsReleased(i: Pick<PracticeFacts, 'releaseMode' | 'status' | 'closesAt' | 'now'>): boolean {
  return i.releaseMode !== 'after_close' || i.status === 'closed' || i.status === 'archived' || (i.closesAt !== null && i.closesAt + PRACTICE_GRACE_MS < i.now)
}

export function practiceAccess(i: PracticeFacts): PracticeAccess {
  if (!i.allowPractice || i.reason === 'not_enrolled') return 'hidden'
  if (i.state === 'in_progress') return 'hidden'
  if (i.status === 'published' && i.opensAt !== null && i.now < i.opensAt) return 'later'
  if (!resultsReleased(i)) return 'later'
  return 'available'
}
