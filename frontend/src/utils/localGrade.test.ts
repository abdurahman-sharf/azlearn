import { choiceCorrect, choiceLetters, fillCorrect } from './localGrade.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(choiceLetters(' c, a '), 'AC', 'sorted letters only')
eq(choiceLetters('AJ'), 'AJ', 'the letters I and J count')
eq(choiceLetters('K1'), '', 'beyond J there is nothing')
eq(choiceLetters(null), '', 'nothing')
eq(choiceCorrect('B', 'b'), true, 'a right letter')
eq(choiceCorrect('AC', 'C,A'), true, 'order does not matter')
eq(choiceCorrect('B', 'A'), false, 'a wrong letter')
eq(choiceCorrect('J', 'J'), true, 'an option I or J can be answered')
eq(choiceCorrect('I', ''), false, 'an empty answer is never right (a key of only I / J used to normalise to nothing on both sides)')
eq(choiceCorrect('J', 'I'), false, 'I and J are different options')
eq(fillCorrect('Cairo', ' cairo '), true, 'case and outer spaces are ignored')
eq(fillCorrect('القاهرة|مصر', 'مصر'), true, 'the second alternative of a key saved from an exam')
eq(fillCorrect('القاهرة|مصر', 'القاهرة'), true, 'the first alternative')
eq(fillCorrect('القاهرة|مصر', 'الجيزة'), false, 'none of them')
eq(fillCorrect('القاهرة|مصر', 'القاهرة|مصر'), false, 'the whole key with the bar is not an answer')
eq(fillCorrect('a|b', ''), false, 'empty is never right')
eq(fillCorrect('', ''), false, 'even against an empty key')
eq(fillCorrect('x', null), false, 'nothing')
console.log('local grade tests ok')
