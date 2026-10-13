import { tabStopIndexes } from './dialogFocus.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

const btn = { isRadio: false, name: '', checked: false }
const radio = (name: string, checked: boolean) => ({ isRadio: true, name, checked })

eq(tabStopIndexes([btn, btn, btn]), [0, 1, 2], 'plain controls are all stops')
eq(tabStopIndexes([]), [], 'nothing')
// the start dialog: details, then a two-radio group, then the buttons
eq(tabStopIndexes([radio('start-mode', true), radio('start-mode', false), btn, btn]), [0, 2, 3], 'graded chosen: the graded radio is the stop')
eq(tabStopIndexes([radio('start-mode', false), radio('start-mode', true), btn, btn]), [1, 2, 3], 'practice chosen: the PRACTICE radio is the stop (Shift+Tab from it must wrap)')
eq(tabStopIndexes([radio('g', false), radio('g', false), btn]), [0, 2], 'nothing chosen: the first radio of the group')
eq(tabStopIndexes([btn, radio('a', false), radio('a', true), radio('b', true), radio('b', false), btn]), [0, 2, 3, 5], 'two groups, one stop each')
eq(tabStopIndexes([radio('', false), radio('', false)]), [0, 1], 'radios without a name are their own groups')
console.log('dialog focus tests ok')
