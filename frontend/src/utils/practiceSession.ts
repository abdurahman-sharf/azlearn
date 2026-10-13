import type { Question, QuestionBank } from '@exameow/shared'
import type { PracticeQuestion } from '@/api/platformExams'
import { checkAnswer, isBlank } from './answerCheck'

// The practice page (/platform/assessments/:id/practice): the questions of an exam WITH their answers, worked through on the
// student's device. Nothing here is sent to the server and nothing is graded. These helpers decide the outcome of each
// question, the summary, and the two local question banks (all questions / my mistakes) the student can keep.

/** What the student did with one question. */
export interface PracticeItemState {
  /** the answer given (letters for choice questions, 'A'/'B' for true/false, text otherwise) */
  answer: string
  /** the model answer was shown (the answer can no longer be changed) */
  revealed: boolean
  /** a short answer (or any question no program can check): the student's own verdict after comparing, null = not decided */
  selfCheck: boolean | null
}

export const blankState = (): PracticeItemState => ({ answer: '', revealed: false, selfCheck: null })

/**
 * `correct` / `incorrect`: checked by the same rules the server grades with (or by the student, for a short answer).
 * `skipped`: nothing was answered. `unchecked`: a written short answer the student has not compared with the model yet.
 */
export type PracticeOutcome = 'correct' | 'incorrect' | 'skipped' | 'unchecked'

export function itemOutcome(q: Pick<PracticeQuestion, 'type' | 'answer'>, st: PracticeItemState): PracticeOutcome {
  if (isBlank(st.answer)) return 'skipped'
  const r = checkAnswer(q, st.answer)
  if (r === 'correct') return 'correct'
  if (r === 'incorrect') return 'incorrect'
  return st.selfCheck === null ? 'unchecked' : st.selfCheck ? 'correct' : 'incorrect'
}

export interface PracticeSummary {
  total: number
  correct: number
  incorrect: number
  skipped: number
  unchecked: number
}

export function summarize(questions: readonly PracticeQuestion[], states: Readonly<Record<string, PracticeItemState>>): PracticeSummary {
  const s: PracticeSummary = { total: questions.length, correct: 0, incorrect: 0, skipped: 0, unchecked: 0 }
  for (const q of questions) s[itemOutcome(q, states[q.id] ?? blankState())]++
  return s
}

/** The questions answered wrongly or skipped, in exam order (a written answer nobody has judged is not a known mistake). */
export function mistakes(questions: readonly PracticeQuestion[], states: Readonly<Record<string, PracticeItemState>>): PracticeQuestion[] {
  return questions.filter((q) => {
    const o = itemOutcome(q, states[q.id] ?? blankState())
    return o === 'incorrect' || o === 'skipped'
  })
}

/**
 * A question the teacher cancelled (0 points, usually because its answer key was wrong) is left out: practising it would
 * check the student against that same faulty key. Returns the rest and how many were left out.
 */
export function usableQuestions(all: readonly PracticeQuestion[]): { usable: PracticeQuestion[]; cancelled: number } {
  const usable = all.filter((q) => q.points > 0)
  return { usable, cancelled: all.length - usable.length }
}

// ── the two local banks ──────────────────────────────────────────────────────────────────────────────────────────────────

/** One stable bank per exam, so saving again updates it instead of adding a copy. */
export const practiceBankId = (assessmentId: string) => `exam-practice-${assessmentId}`
export const mistakesBankId = (assessmentId: string) => `exam-practice-${assessmentId}-mistakes`

/** A question as the local practice mode keeps it. The id stays the exam question's id (wrong-question tracking uses it). */
export function toBankQuestion(q: PracticeQuestion): Question {
  return {
    id: q.id,
    type: q.type as Question['type'],
    stem: q.stem,
    options: [...q.options],
    answer: q.answer,
    analysis: q.analysis ?? '',
    score: q.points,
    ...(q.chapter ? { chapter: q.chapter } : {}),
    ...(q.difficulty ? { difficulty: q.difficulty as Question['difficulty'] } : {}),
  }
}

export function buildBank(id: string, name: string, questions: readonly PracticeQuestion[], createdAt: number): QuestionBank {
  return { id, name, questions: questions.map(toBankQuestion), createdAt, source: 'ai-generated' }
}
