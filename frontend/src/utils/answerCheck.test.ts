import { canonTf, checkAnswer, choiceItems, describeAnswer, fillMatches, isBlank, isTrueAnswer, normFill, normText, normalizeChoice } from './answerCheck.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

// Node's modules are reached through the CommonJS `require` of the test runner (scripts/test-chapters.cjs); the app's
// TypeScript has no Node typings, so only what is used here is described.
interface NodeFs { readFileSync(path: string, encoding: string): string }
interface NodePath { join(...parts: string[]): string }
declare const require: (id: string) => unknown
declare const __dirname: string
const fs = require('node:fs') as NodeFs
const path = require('node:path') as NodePath

// --- the shared fill-blank vectors (the Rust server reads the same file) ----------------------------------------------------------
interface Vector { answer: string; given: string; correct: boolean }
const vectors = JSON.parse(fs.readFileSync(path.join(__dirname, '../../../packages/shared/fixtures/fill-blank-vectors.json'), 'utf8')) as Vector[]
if (vectors.length < 25) throw new Error(`the shared vectors must hold at least 25 cases, found ${vectors.length}`)
for (const v of vectors) {
  eq(fillMatches(v.answer, v.given), v.correct, `vector ${JSON.stringify(v)}`)
  eq(checkAnswer({ type: 'fill_blank', answer: v.answer }, v.given), v.given.trim() === '' ? 'unanswered' : v.correct ? 'correct' : 'incorrect', `checkAnswer ${JSON.stringify(v)}`)
}

// --- the folding pieces ----------------------------------------------------------------------------------------------------------------
eq(normText('  أَحْمَد  '), 'احمد', 'trim, diacritics, alef')
eq(normText('مصطفى'), 'مصطفي', 'alef maqsura becomes yeh')
eq(normText('ABC'), 'abc', 'lowercase')
eq(normFill('٠١٢٣٤٥٦٧٨٩'), '0123456789', 'Arabic-Indic digits')
eq(normFill('۰۱۲۳۴۵۶۷۸۹'), '0123456789', 'Persian digits')
eq(normFill('٣٫٥'), '3.5', 'Arabic decimal separator')
eq(normFill('  ((hello))  '), 'hello', 'edge punctuation and spaces are stripped')
eq(normFill('a - b'), 'a - b', 'inner punctuation stays')
eq(normFill('a   b'), 'a b', 'runs of whitespace (no-break space included) become one space')
eq(normFill('a​b'), 'ab', 'zero-width marks go')
eq(normFill('ة'), 'ة', 'ة is not folded')
eq(normFill('ه'), 'ه', 'ه is not folded')
eq(normFill('؟'), '', 'only punctuation')
eq(normFill('-5'), '-5', 'the minus sign of a number stays')
eq(normFill('\u22125'), '-5', 'the true minus sign is a hyphen')
eq(normFill('(-5)'), '-5', 'brackets around a negative number')
eq(normFill('-.5'), '-.5', 'a sign before a decimal point stays')
eq(normFill('.5'), '.5', 'a leading decimal point stays')
eq(normFill('5.'), '5', 'a trailing full stop goes')
eq(normFill('- القاهرة -'), 'القاهرة', 'a dash that stands apart is decoration')
eq(normFill('-القاهرة'), 'القاهرة', 'a leading hyphen before a letter is decoration')
eq(normFill('Cl-'), 'cl-', 'a hyphen glued to the end of a word stays')
eq(normFill('(NO3-)'), 'no3-', 'brackets around an ion go, the charge stays')
eq(fillMatches('5', '-5'), false, 'a bare number is not a negative one')
eq(fillMatches('-5', '5'), false, 'and the other way round')
eq(fillMatches('?', '?'), true, 'a key of only punctuation matches the same text')
eq(fillMatches('?', '!'), false, 'but not another mark')
eq(fillMatches('القاهرة', '?'), false, 'only punctuation never matches a word')
eq(isBlank('    '), true, 'blank')
eq(isBlank('a'), false, 'not blank')
eq(isBlank(null), true, 'null is blank')

// --- choice questions: letters A-J, sorted, case-insensitive ---------------------------------------------------------------
eq(normalizeChoice(' c, a '), 'AC', 'sorted letters only')
eq(normalizeChoice('jk1'), 'J', 'only A-J')
const single = { type: 'single_choice', answer: 'B' }
eq(checkAnswer(single, 'B'), 'correct', 'right letter')
eq(checkAnswer(single, 'b'), 'correct', 'lowercase letter')
eq(checkAnswer(single, 'A'), 'incorrect', 'wrong letter')
eq(checkAnswer(single, ''), 'unanswered', 'blank')
eq(checkAnswer(single, null), 'unanswered', 'nothing')
const multi = { type: 'multi_choice', answer: 'AC' }
eq(checkAnswer(multi, 'CA'), 'correct', 'order does not matter')
eq(checkAnswer(multi, 'A'), 'incorrect', 'a missing option')
eq(checkAnswer(multi, 'ACD'), 'incorrect', 'an extra option')
eq(checkAnswer({ type: 'multi_choice', answer: 'AJ' }, 'JA'), 'correct', 'the letter J counts')

// --- true / false -----------------------------------------------------------------------------------------------------------------------
eq(canonTf('صحيح'), 'A', 'Arabic true')
eq(canonTf('خطأ'), 'B', 'Arabic false (alef folded)')
eq(canonTf('غير صحيح'), 'B', 'not correct')
eq(canonTf('maybe'), 'maybe', 'unknown text is kept')
eq(isTrueAnswer('A'), true, 'A')
eq(isTrueAnswer('true'), true, 'true in any case')
eq(isTrueAnswer('B'), false, 'B')
eq(isTrueAnswer('正确'), true, 'a long alias')
eq(isTrueAnswer('不正确'), false, 'a negation left over')
const tf = { type: 'true_false', answer: 'A' }
eq(checkAnswer(tf, 'A'), 'correct', 'A is true')
eq(checkAnswer(tf, 'B'), 'incorrect', 'B is false')
eq(checkAnswer(tf, 'صحيح'), 'correct', 'Arabic word')
eq(checkAnswer({ type: 'true_false', answer: 'صحيح' }, 'A'), 'correct', 'the model answer may be the word')
eq(checkAnswer({ type: 'true_false', answer: 'خطأ' }, 'B'), 'correct', 'false word model answer')
eq(checkAnswer({ type: 'true_false', answer: 'B' }, 'A'), 'incorrect', 'wrong side')
eq(checkAnswer(tf, ' '), 'unanswered', 'blank')

// --- short answers are never checked here --------------------------------------------------------------------------------------
eq(checkAnswer({ type: 'short_answer', answer: 'x' }, 'x'), 'manual', 'a teacher grades these')
eq(checkAnswer({ type: 'short_answer', answer: 'x' }, ''), 'manual', 'also when blank (the page decides what blank means)')

// --- writing answers out ---------------------------------------------------------------------------------------------------------
const labels = { trueText: 'صحيح', falseText: 'خطأ' }
eq(choiceItems('CA', ['one', 'two', 'three']), [{ letter: 'A', text: 'one' }, { letter: 'C', text: 'three' }], 'letters sorted, option text beside each')
eq(choiceItems('D', ['one']), [{ letter: 'D', text: '' }], 'a letter without an option keeps an empty text')
eq(describeAnswer({ type: 'single_choice', options: ['x', 'y'] }, 'B', labels), { kind: 'choice', items: [{ letter: 'B', text: 'y' }] }, 'single choice')
eq(describeAnswer({ type: 'true_false', options: [] }, 'A', labels), { kind: 'choice', items: [{ letter: 'A', text: 'صحيح' }] }, 'true/false default words')
eq(describeAnswer({ type: 'true_false', options: [] }, 'B', labels), { kind: 'choice', items: [{ letter: 'B', text: 'خطأ' }] }, 'false')
eq(describeAnswer({ type: 'true_false', options: ['Right', 'Wrong'] }, 'B', labels), { kind: 'choice', items: [{ letter: 'B', text: 'Wrong' }] }, 'own options win')
eq(describeAnswer({ type: 'fill_blank', options: [] }, 'cat|dog', labels, true), { kind: 'text', text: 'cat / dog' }, 'the model answer lists its alternatives')
eq(describeAnswer({ type: 'fill_blank', options: [] }, 'cat|dog', labels), { kind: 'text', text: 'cat|dog' }, 'a student answer is shown as typed')
eq(describeAnswer({ type: 'short_answer', options: [] }, 'free text', labels), { kind: 'text', text: 'free text' }, 'short answer')
eq(describeAnswer({ type: 'single_choice', options: ['x'] }, '  ', labels), { kind: 'none' }, 'blank')
eq(describeAnswer({ type: 'single_choice', options: ['x'] }, null, labels), { kind: 'none' }, 'null')
console.log('answer check tests ok')
