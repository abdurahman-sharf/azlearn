import type { Question } from '@exameow/shared'
import type { CorrectBody } from '@/api/platformExamAdmin'
import { normalizeAnswer } from './examBuilder'

// Pure rules of the answer-key correction dialog: the form it starts from and the request it sends. Kept out of the
// component so a plain script can test them. The server repeats every check; this only spares a round trip.

export type KeyMode = 'set' | 'void'
export interface KeyForm {
  mode: KeyMode
  /** choice questions: the ticked letters ("AC"); true/false: "A" or "B"; the others: free text */
  answer: string
  /** text of the number field ('' = leave as it is); tolerates a real number, which a number input bound with v-model yields */
  score: string | number
  analysis: string
}
/** `nothing` = no field differs from the current key; `answer` / `score` = a value the server would refuse. */
export type KeyProblem = 'nothing' | 'answer' | 'score'
export type KeyBuild = { body: CorrectBody; problem: null } | { body: null; problem: KeyProblem }

const isChoice = (t: string): boolean => t === 'single_choice' || t === 'multi_choice'

/** The score a question counts for (an unset score is 1). */
export const currentScore = (q: Pick<Question, 'score'>): number => q.score ?? 1

/** The question's answer in the form the server stores it. */
export function currentAnswer(q: Pick<Question, 'type' | 'options' | 'answer'>): string {
  return normalizeAnswer(q.type, q.options ?? [], q.answer ?? '') ?? (q.answer ?? '').trim()
}

/** The form a correction starts from: the question as it is now. */
export function initialForm(q: Question): KeyForm {
  return { mode: 'set', answer: currentAnswer(q), score: String(currentScore(q)), analysis: q.analysis ?? '' }
}

/** A voided question counts for nobody (its score is 0). */
export const isVoided = (q: Pick<Question, 'score'>): boolean => q.score === 0

/** Letters of a choice answer as a list of indexes ("AC" → [0, 2]). */
export function lettersToIndexes(answer: string): number[] {
  return [...answer].map((c) => c.charCodeAt(0) - 65).filter((i) => i >= 0 && i < 26)
}
/** Indexes back to the stored form (sorted, no duplicates). */
export function indexesToLetters(indexes: Iterable<number>): string {
  return [...new Set(indexes)].sort((a, b) => a - b).map((i) => String.fromCharCode(65 + i)).join('')
}

/**
 * The request for a form. A `set` sends only the fields that differ from the current key (so nothing is "changed" to the
 * same value) and refuses an empty change; a `void` sends just the mode.
 */
export function buildKeyBody(q: Question, form: KeyForm): KeyBuild {
  if (form.mode === 'void') return { body: { mode: 'void' }, problem: null }
  const body: CorrectBody = { mode: 'set' }

  const nextAnswer = form.answer.trim() === '' ? null : normalizeAnswer(q.type, q.options ?? [], form.answer)
  if (nextAnswer === null) return { body: null, problem: 'answer' }
  if (nextAnswer !== currentAnswer(q)) body.answer = nextAnswer

  const scoreText = String(form.score ?? '').trim()
  if (scoreText !== '') {
    const n = Number(scoreText)
    if (!Number.isFinite(n) || n < 0 || n > 100) return { body: null, problem: 'score' }
    if (n !== currentScore(q)) body.score = n
  }

  const analysis = form.analysis.trim()
  if (analysis !== (q.analysis ?? '').trim()) body.analysis = analysis

  return body.answer === undefined && body.score === undefined && body.analysis === undefined ? { body: null, problem: 'nothing' } : { body, problem: null }
}

/** A choice or true/false answer is picked from the options; the others are typed. */
export const answerIsPicked = (type: string): boolean => isChoice(type) || type === 'true_false'
