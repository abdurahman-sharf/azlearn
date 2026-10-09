import type { Question } from '@exameow/shared'
import { capState, chosenQuestions, filterLocal, selectAllWithin, selectNoneOf, stemKey, takenMask } from './localBankPicker.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const q = (stem: string, p: Record<string, unknown> = {}): Question => ({ id: stem, type: 'single_choice', stem, options: ['a', 'b'], answer: 'A', analysis: '', ...p }) as unknown as Question

// --- the duplicate key folds Arabic variants and whitespace ------------------------------------------------------------
eq(stemKey('  ما   عاصمةُ اليمن؟ '), stemKey('ما عاصمة اليمن؟'), 'diacritics and spaces do not matter')
eq(stemKey('أين الكتاب'), stemKey('اين الكتاب'), 'alef variants fold')
eq(stemKey('A  B') === stemKey('a b'), true, 'case folds')

// --- search ------------------------------------------------------------------------------------------------------------
const bank = [
  q('ما عاصمة اليمن؟', { options: ['صنعاء', 'عدن'], chapter: 'الجغرافيا' }),
  q('اذكر ثلاث مدن', { type: 'short_answer', options: [], answer: 'صنعاء' }),
  q('What is 2+2?', { options: ['3', '4'], chapter: 'Math' }),
  q('ما لون السماء', { options: ['أزرق', 'أحمر'] }),
]
eq(filterLocal(bank, ''), [0, 1, 2, 3], 'empty query matches all')
eq(filterLocal(bank, '   '), [0, 1, 2, 3], 'blank query matches all')
eq(filterLocal(bank, 'صنعاء'), [0, 1], 'matches options and answers')
eq(filterLocal(bank, 'جغرافيا'), [0], 'matches the chapter')
eq(filterLocal(bank, 'math'), [2], 'case-insensitive')
eq(filterLocal(bank, 'اليمن عاصمه'), [], 'every word must match (ه is not ة)')
eq(filterLocal(bank, 'عاصمة اليمن'), [0], 'all words, any order')
eq(filterLocal(bank, 'ازرق'), [3], 'alef with hamza folds in the search')
eq(filterLocal(bank, 'zzz'), [], 'no match')

// --- what is already in the exam ---------------------------------------------------------------------------------------
eq(takenMask(bank, [q('ما  عاصمة اليمن؟'), q('سؤال آخر')]), [true, false, false, false], 'same stem (whitespace folded) is taken')
eq(takenMask(bank, []), [false, false, false, false], 'nothing taken')

// --- the cap ----------------------------------------------------------------------------------------------------------
eq(capState(10, 5, 200), { total: 15, room: 185, over: 0, near: false, full: false }, 'plenty of room')
eq(capState(170, 10, 200), { total: 180, room: 20, over: 0, near: true, full: false }, '90 % is a warning')
eq(capState(179, 0, 200), { total: 179, room: 21, over: 0, near: false, full: false }, 'just under 90 %')
eq(capState(190, 10, 200), { total: 200, room: 0, over: 0, near: true, full: true }, 'exactly full');
eq(capState(195, 10, 200), { total: 205, room: 0, over: 5, near: false, full: true }, 'over the cap is not "near", it is over')

// --- select all / none -------------------------------------------------------------------------------------------------
const taken = [false, true, false, false]
const all = selectAllWithin(new Set(), [0, 1, 2, 3], taken, 10)
eq([...all.next], [0, 2, 3], 'select all skips what is already in the exam')
eq(all.skipped, 0, 'nothing left out')
const limited = selectAllWithin(new Set(), [0, 1, 2, 3], taken, 2)
eq([...limited.next], [0, 2], 'select all stops at the room that is left')
eq(limited.skipped, 1, 'and counts what did not fit')
const keep = selectAllWithin(new Set([3]), [0, 2, 3], taken, 1)
eq([...keep.next].sort(), [0, 3], 'an already ticked question stays and does not use the room twice')
eq(keep.skipped, 1, 'the one that no longer fits')
eq([...selectAllWithin(new Set(), [0, 2], taken, 0).next], [], 'no room: nothing ticked')
// filtered select-all only touches the visible list
eq([...selectAllWithin(new Set([3]), [0], taken, 5).next].sort(), [0, 3], 'select all on a filtered list keeps ticks made elsewhere')
eq([...selectNoneOf(new Set([0, 2, 3]), [0, 2])], [3], 'select none unticks only the visible questions')
eq([...selectNoneOf(new Set([0]), [])], [0], 'select none on an empty view changes nothing')

// --- what is added -----------------------------------------------------------------------------------------------------
const picked = chosenQuestions(bank, new Set([3, 0, 99, -1]))
eq(picked.map((x) => x.stem), ['ما عاصمة اليمن؟', 'ما لون السماء'], 'bank order, out-of-range ignored')
picked[0]!.stem = 'changed'
eq(bank[0]!.stem, 'ما عاصمة اليمن؟', 'the picked questions are copies')

console.log('local bank picker tests ok')
