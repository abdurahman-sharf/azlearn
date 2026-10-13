import { START_FLAG_MAX_AGE_MS, clearStartFlag, consumeStartFlag, readStartFlag, setStartFlag, startFlagKey } from './startFlag.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

function mem(init: Record<string, string> = {}) {
  const data = new Map(Object.entries(init))
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => { data.set(k, v) },
    removeItem: (k: string) => { data.delete(k) },
    keys: () => [...data.keys()],
  }
}
const blocked = {
  getItem: (): string | null => { throw new Error('blocked') },
  setItem: (): void => { throw new Error('blocked') },
  removeItem: (): void => { throw new Error('blocked') },
}

eq(startFlagKey('a1'), 'exameow-start-a1', 'the key the exam page writes')

// --- a fresh confirmation starts one attempt, once -----------------------------------------------------------------------------
const s = mem()
setStartFlag('ex1', 1_000_000, s)
eq(s.keys(), ['exameow-start-ex1'], 'written under the contract key')
eq(readStartFlag('ex1', 1_000_000 + 1000, s), { valid: true, stamp: 1_000_000 }, 'recent: valid, still there after reading')
eq(consumeStartFlag('ex1', 1_000_000 + 1000, s), true, 'consumed while fresh')
eq(s.keys(), [], 'and removed')
eq(consumeStartFlag('ex1', 1_000_000 + 1500, s), false, 'a second page load cannot start another attempt')

// --- too old ----------------------------------------------------------------------------------------------------------------------
setStartFlag('ex1', 2_000_000, s)
eq(readStartFlag('ex1', 2_000_000 + START_FLAG_MAX_AGE_MS - 1, s).valid, true, 'one millisecond before two minutes')
eq(readStartFlag('ex1', 2_000_000 + START_FLAG_MAX_AGE_MS, s).valid, false, 'two minutes: not valid')
eq(consumeStartFlag('ex1', 2_000_000 + START_FLAG_MAX_AGE_MS + 5000, s), false, 'a stale flag starts nothing')
eq(s.keys(), [], 'and is removed')

// --- other exams and bad values ------------------------------------------------------------------------------------------------------
setStartFlag('ex1', 3_000_000, s)
eq(readStartFlag('ex2', 3_000_000, s), { valid: false, stamp: null }, 'a flag is per exam')
const bad = mem({ 'exameow-start-ex3': 'soon', 'exameow-start-ex4': '-5', 'exameow-start-ex5': '' })
eq(consumeStartFlag('ex3', 1, bad), false, 'not a number')
eq(consumeStartFlag('ex4', 1, bad), false, 'negative')
eq(consumeStartFlag('ex5', 1, bad), false, 'empty')
// a device clock a little behind between the two pages
setStartFlag('ex6', 5_000_000 + 3000, s)
eq(readStartFlag('ex6', 5_000_000, s).valid, true, 'three seconds in the future: still fine')
setStartFlag('ex7', 5_000_000 + 60_000, s)
eq(readStartFlag('ex7', 5_000_000, s).valid, false, 'a minute in the future is not a recent confirmation')

// --- blocked storage: the flag still travels through memory ------------------------------------------------------------------------
setStartFlag('ex8', 7_000_000, blocked)
eq(readStartFlag('ex8', 7_000_100, blocked).valid, true, 'memory fallback')
eq(consumeStartFlag('ex8', 7_000_100, blocked), true, 'consumed from memory')
eq(consumeStartFlag('ex8', 7_000_200, blocked), false, 'only once')
setStartFlag('ex9', 8_000_000, null)
clearStartFlag('ex9', null)
eq(readStartFlag('ex9', 8_000_001, null).valid, false, 'cleared')
console.log('start flag tests ok')
