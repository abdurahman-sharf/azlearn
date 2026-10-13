import { markedIndexes, navState, parseMarked, pruneMarked, serializeMarked, toggleMarked, unansweredIndexes } from './reviewMarks.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const qs = [{ id: 'a' }, { id: 'b' }, { id: 'c' }, { id: 'd' }]
eq(unansweredIndexes(qs, { a: 'A', c: '  ', d: 'x' }), [1, 2], 'blank and missing answers are unanswered')
eq(unansweredIndexes(qs, { a: 'A', b: 'B', c: 'C', d: 'D' }), [], 'all answered')
eq(unansweredIndexes([], {}), [], 'no questions')
eq(markedIndexes(qs, new Set(['d', 'b', 'zz'])), [1, 3], 'in question order, unknown ids ignored')

const m0 = new Set<string>()
const m1 = toggleMarked(m0, 'a')
eq([...m1], ['a'], 'marked')
eq(m0.size, 0, 'the original set is untouched')
eq([...toggleMarked(m1, 'a')], [], 'unmarked again')
eq([...toggleMarked(m1, 'b')].sort(), ['a', 'b'], 'two marks')

eq([...pruneMarked(new Set(['a', 'gone']), qs)], ['a'], 'marks of removed questions are dropped')
eq(serializeMarked(new Set(['a', 'c'])), '["a","c"]', 'stored as a list')
eq([...parseMarked('["a","c",3,null]')], ['a', 'c'], 'only text entries')
eq([...parseMarked('{"a":1}')], [], 'an object is not a list')
eq([...parseMarked('garbage')], [], 'corrupt text')
eq([...parseMarked(null)], [], 'nothing stored')
eq(navState('a', { a: 'A' }), 'answered', 'answered')
eq(navState('a', { a: ' ' }), 'unanswered', 'blank')
eq(navState('a', {}), 'unanswered', 'missing')
console.log('review marks tests ok')
