import type { Question } from '@exameow/shared'
import { answerIsPicked, buildKeyBody, currentAnswer, indexesToLetters, initialForm, isVoided, lettersToIndexes } from './answerKey.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const q = (p: Record<string, unknown> = {}): Question => ({ id: 'q1', type: 'single_choice', stem: 'س', options: ['a', 'b', 'c', 'd'], answer: 'B', analysis: 'لأن', score: 2, ...p }) as unknown as Question

// --- the starting form is the question as it is --------------------------------------------------------------------------
eq(initialForm(q()), { mode: 'set', answer: 'B', score: '2', analysis: 'لأن' }, 'starts from the current key')
eq(initialForm(q({ score: undefined })).score, '1', 'an unset score is 1')
eq(currentAnswer(q({ type: 'multi_choice', answer: 'c, a' })), 'AC', 'a stored multi answer is shown sorted')
eq(isVoided(q({ score: 0 })), true, 'score 0 = voided')
eq(isVoided(q({ score: undefined })), false, 'unset is not voided')
eq(isVoided(q({ score: 1 })), false, 'one is not voided')

// --- nothing changed -------------------------------------------------------------------------------------------------------
eq(buildKeyBody(q(), initialForm(q())), { body: null, problem: 'nothing' }, 'no change is refused before the round trip')

// --- one field at a time -----------------------------------------------------------------------------------------------------
eq(buildKeyBody(q(), { ...initialForm(q()), answer: 'C' }), { body: { mode: 'set', answer: 'C' }, problem: null }, 'only the answer')
eq(buildKeyBody(q(), { ...initialForm(q()), score: '5' }), { body: { mode: 'set', score: 5 }, problem: null }, 'only the score')
eq(buildKeyBody(q(), { ...initialForm(q()), score: '0.5' }), { body: { mode: 'set', score: 0.5 }, problem: null }, 'half points')
eq(buildKeyBody(q(), { ...initialForm(q()), analysis: ' شرح جديد ' }), { body: { mode: 'set', analysis: 'شرح جديد' }, problem: null }, 'only the analysis, trimmed')
eq(buildKeyBody(q(), { ...initialForm(q()), analysis: '' }), { body: { mode: 'set', analysis: '' }, problem: null }, 'the explanation can be cleared')
eq(buildKeyBody(q(), { ...initialForm(q()), answer: 'D', score: '3', analysis: 'x' }).body, { mode: 'set', answer: 'D', score: 3, analysis: 'x' }, 'all three')

// --- answers are normalised like the server does ------------------------------------------------------------------------------
eq(buildKeyBody(q({ type: 'multi_choice', answer: 'A' }), { ...initialForm(q({ type: 'multi_choice', answer: 'A' })), answer: 'CA' }).body, { mode: 'set', answer: 'AC' }, 'multi: sorted letters')
eq(buildKeyBody(q(), { ...initialForm(q()), answer: 'E' }), { body: null, problem: 'answer' }, 'a letter beyond the options')
eq(buildKeyBody(q(), { ...initialForm(q()), answer: 'AB' }), { body: null, problem: 'answer' }, 'single choice needs exactly one')
eq(buildKeyBody(q(), { ...initialForm(q()), answer: '' }), { body: null, problem: 'answer' }, 'an empty answer')
const tf = q({ type: 'true_false', options: ['صحيح', 'خطأ'], answer: 'A' })
eq(buildKeyBody(tf, { ...initialForm(tf), answer: 'B' }).body, { mode: 'set', answer: 'B' }, 'true/false A→B')
eq(buildKeyBody(tf, { ...initialForm(tf), answer: 'خطأ' }).body, { mode: 'set', answer: 'B' }, 'true/false by word')
const fill = q({ type: 'fill_blank', options: [], answer: 'صنعاء' })
eq(buildKeyBody(fill, { ...initialForm(fill), answer: ' صنعاء|اليمن ' }).body, { mode: 'set', answer: 'صنعاء|اليمن' }, 'fill: alternatives, trimmed')
eq(buildKeyBody(fill, { ...initialForm(fill), answer: ' صنعاء ' }), { body: null, problem: 'nothing' }, 'fill: the same text with spaces is no change')
const short = q({ type: 'short_answer', options: [], answer: 'مرجع' })
eq(buildKeyBody(short, { ...initialForm(short), answer: 'مرجع جديد' }).body, { mode: 'set', answer: 'مرجع جديد' }, 'short: free text')

// --- scores -------------------------------------------------------------------------------------------------------------------
eq(buildKeyBody(q(), { ...initialForm(q()), score: '101' }), { body: null, problem: 'score' }, 'over 100')
eq(buildKeyBody(q(), { ...initialForm(q()), score: '-1' }), { body: null, problem: 'score' }, 'negative')
eq(buildKeyBody(q(), { ...initialForm(q()), score: 'abc' }), { body: null, problem: 'score' }, 'not a number')
eq(buildKeyBody(q(), { ...initialForm(q()), score: '' }), { body: null, problem: 'nothing' }, 'a blank score field leaves the score alone')
eq(buildKeyBody(q({ score: 0 }), { ...initialForm(q({ score: 0 })), score: '2' }).body, { mode: 'set', score: 2 }, 'a voided question counts again with new points')

// a number input bound with v-model stores a real number once it is edited (not a string): it must not throw
eq(buildKeyBody(q(), { ...initialForm(q()), score: 5 as unknown as string }), { body: { mode: 'set', score: 5 }, problem: null }, 'a numeric score (v-model on a number input)')
eq(buildKeyBody(q(), { ...initialForm(q()), score: 0 as unknown as string }), { body: { mode: 'set', score: 0 }, problem: null }, 'a numeric zero is a real value, not an empty field')
eq(buildKeyBody(q(), { ...initialForm(q()), score: 101 as unknown as string }), { body: null, problem: 'score' }, 'a numeric score over 100')
eq(buildKeyBody(q(), { ...initialForm(q()), score: null as unknown as string }), { body: null, problem: 'nothing' }, 'a cleared field may arrive as null')
eq(buildKeyBody(q(), { ...initialForm(q()), score: 2 as unknown as string }), { body: null, problem: 'nothing' }, 'the current numeric score is no change')

// --- void ---------------------------------------------------------------------------------------------------------------------
eq(buildKeyBody(q(), { ...initialForm(q()), mode: 'void', answer: 'zzz' }), { body: { mode: 'void' }, problem: null }, 'void sends the mode only, whatever else is typed')

// --- letters <-> indexes ------------------------------------------------------------------------------------------------------
eq(lettersToIndexes('ACE'), [0, 2, 4], 'letters to indexes')
eq(lettersToIndexes(''), [], 'no letters')
eq(indexesToLetters([2, 0, 2]), 'AC', 'indexes to sorted unique letters')
eq(indexesToLetters([]), '', 'no indexes')
eq([answerIsPicked('single_choice'), answerIsPicked('multi_choice'), answerIsPicked('true_false'), answerIsPicked('fill_blank'), answerIsPicked('short_answer')], [true, true, true, false, false], 'which types are picked, not typed')

console.log('answer key tests ok')
