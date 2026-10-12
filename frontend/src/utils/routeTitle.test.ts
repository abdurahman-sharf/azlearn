import { composeTitle, focusStillOurs, shouldMoveFocus, titleKeyOf } from './routeTitle.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(composeTitle('My exams', 'azlearn'), 'My exams · azlearn', 'page and brand')
eq(composeTitle('', 'azlearn'), 'azlearn', 'no page title: the brand')
eq(composeTitle(undefined, 'azlearn'), 'azlearn', 'an unknown key translates to undefined: the brand')
eq(composeTitle('  ', ' azlearn '), 'azlearn', 'blank page title, trimmed brand')
eq(composeTitle('azlearn', 'azlearn'), 'azlearn', 'never "azlearn · azlearn"')
eq(composeTitle('Page', ''), 'Page', 'no brand')

eq(titleKeyOf({ titleKey: 'stexTitle' }), 'stexTitle', 'declared key')
eq(titleKeyOf({ title: 'Old' }), null, 'only titleKey counts')
eq(titleKeyOf({ titleKey: 3 }), null, 'not a string')
eq(titleKeyOf(undefined), null, 'no meta')

eq(shouldMoveFocus({ path: '/platform/my-exams' }, { path: '/platform/student' }, false), true, 'another page')
eq(shouldMoveFocus({ path: '/platform/my-exams' }, { path: '/platform/my-exams' }, false), false, 'query-only change (a filter)')
eq(shouldMoveFocus({ path: '/platform/my-exams' }, { path: '/' }, true), false, 'first load')

const body = { isConnected: true }
const main = { isConnected: true }
const link = { isConnected: true }
const other = { isConnected: true }
eq(focusStillOurs(null, link, body, main), true, 'nothing focused')
eq(focusStillOurs(body, link, body, main), true, 'the body')
eq(focusStillOurs(main, link, body, main), true, 'the main landmark')
eq(focusStillOurs(link, link, body, main), true, 'still the link that was clicked')
eq(focusStillOurs({ isConnected: false }, link, body, main), true, 'the focused element left the page')
eq(focusStillOurs(other, link, body, main), false, 'the person moved on (e.g. started typing in a search box)')
console.log('route title tests ok')
