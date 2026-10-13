import { mergeKeys, mergeRows } from './bookmarkMerge.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const sorted = (s: Set<string>) => [...s].sort()

// the server's list is overlaid with what this device changed while it was being fetched
eq(sorted(mergeKeys(['post:a', 'course:b'], new Map())), ['course:b', 'post:a'], 'no local change: the server list as is')
eq(sorted(mergeKeys(['post:a'], new Map([['post:new', true]]))), ['post:a', 'post:new'], 'a save the answer does not know about stays saved')
eq(sorted(mergeKeys(['post:a', 'course:b'], new Map([['post:a', false]]))), ['course:b'], 'a removal the answer still lists stays removed')
eq(sorted(mergeKeys(['post:a'], new Map([['post:a', true]]))), ['post:a'], 'a save the answer already has changes nothing')
eq(sorted(mergeKeys([], new Map([['post:x', false]]))), [], 'removing what is not there is harmless')
eq(sorted(mergeKeys(['post:a'], new Map([['post:a', false], ['post:a2', true]]))), ['post:a2'], 'several changes at once')
const input = new Set(['post:a'])
mergeKeys(input, new Map([['post:z', true]]))
eq([...input], ['post:a'], 'the input is not modified')

const rows = [{ k: 'post:a' }, { k: 'course:b' }, { k: 'post:c' }]
const key = (r: { k: string }) => r.k
eq(mergeRows(rows, key, new Map()).map(key), ['post:a', 'course:b', 'post:c'], 'no change: every row')
eq(mergeRows(rows, key, new Map([['course:b', false]])).map(key), ['post:a', 'post:c'], 'a row this device removed meanwhile is dropped')
eq(mergeRows(rows, key, new Map([['course:b', true], ['post:zz', true]])).map(key), ['post:a', 'course:b', 'post:c'], 'a save adds no row (its details come with the next read)')
eq(rows.length, 3, 'the input is not modified')
console.log('bookmarkMerge tests ok')
