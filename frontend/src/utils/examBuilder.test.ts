import type { Question } from '@exameow/shared'
import { adoptQuestions, fromLocalInput, move, normalizeAnswer, toLocalInput, totals, validateQuestion } from './examBuilder.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const o = (n: number) => Array.from({ length: n }, (_, i) => `o${i}`)

// answers normalise exactly like the server's bank/exam rules
eq(normalizeAnswer('single_choice', o(4), 'b'), 'B', 'lower-case letter')
eq(normalizeAnswer('single_choice', o(4), 'B. القاهرة'), 'B', 'letter followed by text')
eq(normalizeAnswer('multi_choice', o(4), 'C, a'), 'AC', 'sorted letters')
eq(normalizeAnswer('multi_choice', o(4), 'CA،A'), 'AC', 'arabic comma + duplicates')
eq(normalizeAnswer('single_choice', o(3), 'D'), null, 'letter beyond options')
eq(normalizeAnswer('single_choice', o(4), 'AB'), null, 'single choice needs exactly one')
eq(normalizeAnswer('single_choice', o(4), 'القاهرة'), null, 'text instead of a letter')
eq(normalizeAnswer('multi_choice', o(4), ''), null, 'empty')
for (const t of ['True', 'صحيح', 'صَحِيح', 'A', 'نعم', '对']) eq(normalizeAnswer('true_false', [], t), 'A', t)
for (const f of ['false', 'خطأ', 'خطا', 'B', 'لا', '错误']) eq(normalizeAnswer('true_false', [], f), 'B', f)
eq(normalizeAnswer('true_false', [], 'ربما'), null, 'tf garbage')
eq(normalizeAnswer('fill_blank', [], '  صنعاء|اليمن '), 'صنعاء|اليمن', 'fill keeps alternatives')

const q = (p: Partial<Question>): Question => ({ id: 'x', type: 'single_choice', stem: 'س', options: ['a', 'b'], answer: 'A', analysis: '', ...p } as Question)
eq(validateQuestion(q({})), null, 'valid question')
eq(validateQuestion(q({ stem: ' ' })), 'stem', 'empty stem')
eq(validateQuestion(q({ options: ['a'] })), 'options', 'one option')
eq(validateQuestion(q({ answer: 'Z' })), 'answer', 'bad letter')
eq(validateQuestion(q({ type: 'short_answer' as Question['type'], options: [], answer: '' })), 'answer', 'empty reference answer')
eq(validateQuestion(q({ score: 101 })), 'score', 'score too high')
eq(validateQuestion(q({ type: 'true_false' as Question['type'], options: [], answer: 'True' })), null, 'tf accepted before normalising')

// AI output becomes clean exam questions
let n = 0
const adopted = adoptQuestions(
  [
    { id: 'dup', type: 'true_false', stem: 'الأرض كروية', options: ['True', 'False'], answer: 'False', analysis: '' } as Question,
    { id: 'dup', type: 'multi_choice', stem: 'اختر', options: ['a', 'b', 'c'], answer: 'c, a', analysis: 'ش', chapter: 'الوحدة 1', difficulty: 'hard' } as Question,
    { id: 'dup', type: 'true_false', stem: 'بلا خيارات', options: [], answer: 'صحيح', analysis: '' } as Question,
  ],
  () => `q${++n}`,
  (x) => (x.stem === 'اختر' ? 'bank-9' : undefined),
)
eq(adopted.map((a) => a.id), ['q1', 'q2', 'q3'], 'fresh unique ids (AI ids can collide across batches)')
eq([adopted[0]!.answer, adopted[1]!.answer, adopted[2]!.answer], ['B', 'AC', 'A'], 'answers normalised')
eq(adopted[2]!.options, ['صحيح', 'خطأ'], 'default true/false options')
eq([adopted[1]!.chapter, adopted[1]!.difficulty, adopted[1]!.src, adopted[0]!.score], ['الوحدة 1', 'hard', 'bank-9', 1], 'metadata kept, default score 1')

eq(totals([q({ score: 2 }), q({ score: 0.5, type: 'fill_blank' as Question['type'] }), q({})]), { points: 3.5, byType: { single_choice: 2, fill_blank: 1 } }, 'totals')
eq(move([1, 2, 3], 0, 1), [2, 1, 3], 'move down')
eq(move([1, 2, 3], 0, -1), [1, 2, 3], 'cannot move above the first')
eq(move([1, 2, 3], 2, 1), [1, 2, 3], 'cannot move below the last')
eq(fromLocalInput(''), null, 'empty datetime')
eq(fromLocalInput(toLocalInput(1_800_000_000_000)), 1_800_000_000_000 - (1_800_000_000_000 % 60000), 'datetime round trip (minute precision)')
ok(toLocalInput(null) === '', 'unset datetime')
console.log('exam builder tests ok')
