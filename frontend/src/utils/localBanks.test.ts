import { hasLocalQuestions, LOCAL_QUESTION_CLEAR_KEYS } from './localBanks.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const store = (o: Record<string, string>) => ({ getItem: (k: string) => (k in o ? (o[k] as string) : null) })

eq(hasLocalQuestions(null), false, 'no storage')
eq(hasLocalQuestions(undefined), false, 'undefined storage')
eq(hasLocalQuestions(store({})), false, 'nothing stored')
eq(hasLocalQuestions(store({ 'exameow-banks': '[]' })), false, 'empty bank list')
eq(hasLocalQuestions(store({ 'exameow-banks': ' [] ' })), false, 'empty bank list with spaces')
eq(hasLocalQuestions(store({ 'exameow-banks': '' })), false, 'empty string')
eq(hasLocalQuestions(store({ 'exameow-banks': 'null' })), false, 'null')
eq(hasLocalQuestions(store({ 'exameow-banks': '[{"id":"b1","name":"x","questions":[]}]' })), true, 'one bank')
eq(hasLocalQuestions(store({ 'exameow-questions': '[{"id":"q1"}]' })), true, 'generated questions only')
eq(hasLocalQuestions(store({ 'exameow-banks': '[]', 'exameow-questions': '[{"id":"q1"}]' })), true, 'empty banks but generated questions')
eq(hasLocalQuestions(store({ 'exameow-banks': 'not json at all' })), true, 'unparseable but non-empty: ask anyway')
// keys that are not question content never trigger the question
eq(hasLocalQuestions(store({ 'exameow-sourcefile': 'file.pdf', 'exameow-wrong-questions': '[{"questionId":"1"}]', 'exameow-practice-history': '{"2025-01-01":{}}' })), false, 'unrelated keys')
// blocked storage
eq(hasLocalQuestions({ getItem: () => { throw new Error('blocked') } }), false, 'throwing storage')

const clearKeys: readonly string[] = LOCAL_QUESTION_CLEAR_KEYS
eq(clearKeys.includes('exameow-banks'), true, 'clear removes banks')
eq(clearKeys.includes('exameow-questions'), true, 'clear removes generated questions')
eq(clearKeys.includes('exameow-wrong-questions'), false, 'wrong-question counters are not question content')

console.log('local banks tests ok')
