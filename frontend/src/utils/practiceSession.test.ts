import type { PracticeQuestion } from '@/api/platformExams'
import { blankState, buildBank, itemOutcome, mistakes, mistakesBankId, practiceBankId, summarize, toBankQuestion, usableQuestions } from './practiceSession.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const q = (id: string, type: string, answer: string, extra: Partial<PracticeQuestion> = {}): PracticeQuestion =>
  ({ id, type, stem: `stem ${id}`, options: type === 'single_choice' || type === 'multi_choice' ? ['x', 'y', 'z'] : [], points: 1, answer, analysis: null, chapter: null, difficulty: null, ...extra })

const questions = [
  q('q1', 'single_choice', 'B'),
  q('q2', 'multi_choice', 'AC'),
  q('q3', 'true_false', 'A'),
  q('q4', 'fill_blank', 'cat|dog'),
  q('q5', 'short_answer', 'a model answer'),
]
const st = (answer: string, selfCheck: boolean | null = null) => ({ answer, revealed: true, selfCheck })

// --- one question ------------------------------------------------------------------------------------------------------------------
eq(itemOutcome(questions[0]!, st('B')), 'correct', 'the right letter')
eq(itemOutcome(questions[0]!, st('A')), 'incorrect', 'the wrong letter')
eq(itemOutcome(questions[0]!, st('')), 'skipped', 'nothing answered')
eq(itemOutcome(questions[0]!, blankState()), 'skipped', 'the untouched state')
eq(itemOutcome(questions[1]!, st('CA')), 'correct', 'a multi-choice answer in any order')
eq(itemOutcome(questions[3]!, st(' Dog. ')), 'correct', 'a fill-blank alternative, normalised')
eq(itemOutcome(questions[3]!, st('bird')), 'incorrect', 'a wrong fill-blank')
eq(itemOutcome(questions[4]!, st('my attempt')), 'unchecked', 'a written short answer waits for the student to compare')
eq(itemOutcome(questions[4]!, st('my attempt', true)), 'correct', 'self-checked right')
eq(itemOutcome(questions[4]!, st('my attempt', false)), 'incorrect', 'self-checked wrong')
eq(itemOutcome(questions[4]!, st('', true)), 'skipped', 'a verdict without an answer is still skipped')

// --- summary and mistakes ------------------------------------------------------------------------------------------------------------
const states = { q1: st('B'), q2: st('A'), q3: st(''), q4: st('dog'), q5: st('text') }
eq(summarize(questions, states), { total: 5, correct: 2, incorrect: 1, skipped: 1, unchecked: 1 }, 'counts add up to the total')
eq(summarize(questions, {}), { total: 5, correct: 0, incorrect: 0, skipped: 5, unchecked: 0 }, 'nothing done')
eq(mistakes(questions, states).map((x) => x.id), ['q2', 'q3'], 'wrong and skipped, in exam order; an unchecked written answer is not a known mistake')
eq(mistakes(questions, { ...states, q5: st('text', false) }).map((x) => x.id), ['q2', 'q3', 'q5'], 'a self-checked miss joins')

// --- cancelled questions ---------------------------------------------------------------------------------------------------------------
const mixed = [q('a', 'single_choice', 'A'), q('b', 'single_choice', 'A', { points: 0 }), q('c', 'single_choice', 'A')]
eq(usableQuestions(mixed).usable.map((x) => x.id), ['a', 'c'], 'a 0-point question is left out')
eq(usableQuestions(mixed).cancelled, 1, 'and counted')
eq(usableQuestions([]).cancelled, 0, 'none')

// --- banks -------------------------------------------------------------------------------------------------------------------------------
eq(practiceBankId('ex1'), 'exam-practice-ex1', 'stable bank id')
eq(mistakesBankId('ex1'), 'exam-practice-ex1-mistakes', 'stable mistakes bank id')
const bq = toBankQuestion(q('q9', 'single_choice', 'B', { analysis: 'because', chapter: 'Ch 1', difficulty: 'hard', points: 2 }))
eq(bq, { id: 'q9', type: 'single_choice', stem: 'stem q9', options: ['x', 'y', 'z'], answer: 'B', analysis: 'because', score: 2, chapter: 'Ch 1', difficulty: 'hard' }, 'all the question fields travel')
eq(toBankQuestion(q('q8', 'fill_blank', 'a|b')).analysis, '', 'no analysis becomes empty text')
eq('chapter' in toBankQuestion(q('q8', 'fill_blank', 'a|b')), false, 'no empty chapter key')
const bank = buildBank('exam-practice-ex1', 'Exam', questions, 123)
eq([bank.id, bank.name, bank.createdAt, bank.source, bank.questions.length], ['exam-practice-ex1', 'Exam', 123, 'ai-generated', 5], 'the bank')
console.log('practice session tests ok')
