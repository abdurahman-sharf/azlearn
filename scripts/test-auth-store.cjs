// The session store's offline / recovery state machine (frontend/src/stores/auth.ts), run against a scripted `platformFetch`.
// Required by scripts/test-chapters.cjs AFTER it installed the TypeScript require hook and the `@/` alias, so the store is
// the real file; only the network layer (`@/lib/platformApi`, which reads `import.meta.env`) is replaced by a stub.
//
// What it pins (each one was a real failure mode of the first-load / reconnect path every role goes through):
//  * a network failure or a server error at start-up is NOT cached: the next init() asks again, the token is kept, and the
//    listeners that retry on `online` / tab visible are registered;
//  * a real 401 still ends the session (token removed, `consumeSessionLost()` true exactly once), with no listeners left;
//  * retry() reads the profile, bumps `epoch` (the shell remounts the page), calls the recovered handler and removes the
//    listeners; signing in / out / clearing the session always leaves a clean state;
//  * concurrent init() calls share one request.
const assert = require('node:assert/strict')
const path = require('node:path')

const root = path.resolve(__dirname, '..')
const fe = (p) => path.join(root, 'frontend', p)

// ── the stubbed network layer ────────────────────────────────────────────────────────────────────────────────────────────
class PlatformError extends Error {
  constructor(code, status, data = {}) {
    super(code)
    this.code = code
    this.status = status
    this.data = data
  }
}
const net = { token: 'tok', script: [], calls: 0, gate: null }
const stub = {
  PlatformError,
  platformEnabled: true,
  getToken: () => net.token,
  setToken: (t) => { net.token = t },
  platformFetch: async (p) => {
    assert.equal(p, '/me', 'the store only asks for the profile here')
    net.calls++
    if (net.gate) await net.gate
    const next = net.script.shift()
    assert.ok(next, 'the script has an answer for every request')
    if (next instanceof Error) throw next
    return next
  },
}
const apiPath = require.resolve('@/lib/platformApi')
require.cache[apiPath] = { id: apiPath, filename: apiPath, loaded: true, exports: stub, children: [], paths: [] }

// ── a minimal window / document that records listeners ───────────────────────────────────────────────────────────────────
const listeners = { window: new Map(), document: new Map() }
const target = (name) => ({
  addEventListener: (type, fn) => { listeners[name].set(type, fn) },
  removeEventListener: (type, fn) => { if (listeners[name].get(type) === fn) listeners[name].delete(type) },
})
const pinia = require(fe('node_modules/pinia'))
// Pinia decides at import time whether it runs in a browser (devtools); the store itself checks `window` when it listens.
globalThis.window = target('window')
globalThis.document = { ...target('document'), visibilityState: 'visible' }
pinia.setActivePinia(pinia.createPinia())
const { useAuthStore, setRecoveredHandler } = require(fe('src/stores/auth.ts'))

const profile = (over = {}) => ({ id: 'u1', email: 'a@x.com', full_name: 'A', role: 'student', institution_type: null, status: 'active', status_reason: null, ...over })
const network = () => new PlatformError('network', 0)
const listening = () => listeners.window.has('online') && listeners.document.has('visibilitychange')
const silent = () => listeners.window.size === 0 && listeners.document.size === 0
/** a fresh store (the stores are singletons per pinia: a new pinia is a new session) */
function fresh(token = 'tok') {
  pinia.setActivePinia(pinia.createPinia())
  listeners.window.clear()
  listeners.document.clear()
  Object.assign(net, { token, script: [], calls: 0, gate: null })
  setRecoveredHandler(null)
  return useAuthStore()
}

async function run() {
  // 1. no stored session: ready, nothing to retry
  let a = fresh(null)
  await a.init()
  assert.deepEqual([a.ready, a.offline, a.isLoggedIn, net.calls], [true, false, false, 0])
  assert.ok(silent(), 'no listeners without a failure')

  // 2. a network failure keeps the token, goes offline, listens - and is NOT cached
  a = fresh()
  net.script.push(network(), profile())
  await a.init()
  assert.deepEqual([a.ready, a.offline, a.isLoggedIn, net.token], [true, true, false, 'tok'], 'offline with the session kept')
  assert.ok(listening(), 'it will retry when the connection or the tab returns')
  await a.init()
  assert.equal(net.calls, 2, 'the failure was not cached: the second init() asked again')
  assert.deepEqual([a.offline, a.isLoggedIn], [false, true])
  assert.ok(silent(), 'recovered: the listeners are gone')

  // 3. a server error (500) is the same as no network; a success afterwards is cached
  a = fresh()
  net.script.push(new PlatformError('server_error', 500), profile({ role: 'teacher' }))
  await a.init()
  assert.deepEqual([a.offline, net.token], [true, 'tok'], '5xx does not end the session')
  await a.init()
  await a.init()
  assert.equal(net.calls, 2, 'once the profile is read, init() is cached for good')
  assert.equal(a.role, 'teacher')

  // 4. a real 401 ends the session, once, with no listeners
  a = fresh()
  net.script.push(new PlatformError('unauthorized', 401))
  await a.init()
  assert.deepEqual([a.offline, a.isLoggedIn, net.token], [false, false, null], '401: the stored token is dropped')
  assert.equal(a.consumeSessionLost(), true, 'the sign-in page can say the session expired')
  assert.equal(a.consumeSessionLost(), false, '... once')
  assert.ok(silent())
  await a.init()
  assert.equal(net.calls, 1, 'a 401 answer is final (no token left to try)')

  // 5. retry(): profile read, epoch bumped, handler called, listeners removed
  a = fresh()
  let recovered = 0
  setRecoveredHandler(() => { recovered++ })
  net.script.push(network(), network(), profile())
  await a.init()
  const epoch0 = a.epoch
  await a.retry()
  assert.deepEqual([a.offline, recovered, a.epoch], [true, 0, epoch0], 'still offline: nothing recovered, no remount')
  assert.ok(listening(), 'still listening')
  await a.retry()
  assert.deepEqual([a.offline, a.isLoggedIn, recovered, a.epoch], [false, true, 1, epoch0 + 1], 'recovered once')
  assert.ok(silent())
  await a.retry()
  assert.equal(net.calls, 3, 'retry() does nothing when we are not offline')

  // 6. the browser events trigger the retry; a hidden tab does not
  a = fresh()
  net.script.push(network(), network(), profile())
  await a.init()
  document.visibilityState = 'hidden'
  listeners.document.get('visibilitychange')()
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(net.calls, 1, 'a tab going to the background does not ask')
  document.visibilityState = 'visible'
  listeners.document.get('visibilitychange')()
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(net.calls, 2, 'the tab coming back asks')
  listeners.window.get('online')()
  await new Promise((r) => setTimeout(r, 0))
  assert.deepEqual([net.calls, a.isLoggedIn, a.offline], [3, true, false], '`online` asks, and the profile arrives')

  // 7. concurrent init() calls share one request
  a = fresh()
  let release
  net.gate = new Promise((r) => { release = r })
  net.script.push(profile())
  const both = Promise.all([a.init(), a.init()])
  release()
  await both
  assert.equal(net.calls, 1, 'one request for two callers')

  // 8. signing out / clearing the session leaves a clean state
  for (const leave of [(s) => s.clearSession(), (s) => s.signOut()]) {
    a = fresh()
    net.script.push(network())
    await a.init()
    assert.ok(listening())
    const logout = stub.platformFetch
    stub.platformFetch = async (p, init) => (p === '/logout' ? undefined : logout(p, init))
    await leave(a)
    stub.platformFetch = logout
    assert.deepEqual([a.offline, a.isLoggedIn], [false, false])
    assert.ok(silent(), 'no listener outlives the session')
  }

  // 9. signing in while offline-flagged resets it
  a = fresh()
  net.script.push(network())
  await a.init()
  const original = stub.platformFetch
  stub.platformFetch = async (p) => (p === '/login' ? { token: 'new', user: profile() } : original(p))
  await a.signIn('a@x.com', 'password123')
  stub.platformFetch = original
  assert.deepEqual([a.offline, a.isLoggedIn, net.token], [false, true, 'new'])
  assert.ok(silent())
  await a.init()
  assert.equal(net.calls, 1, 'after a sign-in init() has nothing left to do')

  console.log('auth store offline / recovery tests passed')
}

module.exports = { done: run() }
