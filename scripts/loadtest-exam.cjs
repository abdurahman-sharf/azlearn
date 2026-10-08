// Load test: N (default 300) students take one shuffled 40-question exam at the same time.
// Run against a release build:   cargo build --release -p exameow-server && node scripts/loadtest-exam.cjs
// Starts its own server on port 3097 with a throw-away database, checks correctness (layouts, no leaks, exact
// scores through the shuffles, resume, autosave, pass count) and prints latency percentiles per phase.
const { spawn, execFileSync } = require('child_process'); const fs = require('fs'); const os = require('os'); const path = require('path')
const ROOT = path.resolve(__dirname, '..'), PORT = 3097, API = `http://localhost:${PORT}/api/platform`
const DIR = path.join(os.tmpdir(), 'exameow-loadtest'); fs.rmSync(DIR, { recursive: true, force: true }); fs.mkdirSync(DIR, { recursive: true })
const N = Number(process.env.N || 300)
let proc
const call = async (method, path, token, body, ip) => {
  const t0 = performance.now()
  const r = await fetch(API + path, { method, headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: 'Bearer ' + token } : {}), ...(ip ? { 'X-Forwarded-For': ip } : {}) }, body: body ? JSON.stringify(body) : undefined })
  const text = await r.text(); let json = null; try { json = JSON.parse(text) } catch {}
  return { status: r.status, json, ms: performance.now() - t0 }
}
const pool = async (items, size, fn) => { const out = new Array(items.length); let i = 0; await Promise.all(Array.from({ length: size }, async () => { while (i < items.length) { const k = i++; out[k] = await fn(items[k], k) } })); return out }
const norm = o => JSON.stringify(Object.entries(o).sort(([a], [b]) => a.localeCompare(b)))
const pct = (a, p) => { const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.floor(s.length * p))] }
const rssMb = () => Math.round(Number(fs.readFileSync(`/proc/${proc.pid}/status`, 'utf8').match(/VmRSS:\s+(\d+)/)?.[1]) / 1024)
const stats = (label, rs) => { const ms = rs.map(r => r.ms); const bad = rs.filter(r => r.status >= 400).length; console.log(`${label.padEnd(34)} n=${String(rs.length).padStart(4)}  p50=${pct(ms, .5).toFixed(0).padStart(5)}ms  p95=${pct(ms, .95).toFixed(0).padStart(5)}ms  max=${Math.max(...ms).toFixed(0).padStart(5)}ms  errors=${bad}`); return bad }
let failures = 0; const ok = (label, cond) => { console.log((cond ? 'PASS' : 'FAIL') + '  ' + label); if (!cond) failures++ }
;(async () => {
  proc = spawn(`${ROOT}/target/release/exameow-server`, [], { cwd: ROOT, env: { ...process.env, PORT: String(PORT), EXAM_DB_PATH: DIR + '/e.db', PLATFORM_DB_PATH: DIR + '/p.db', PLATFORM_FILES_DIR: DIR + '/files', ADMIN_TOKEN_FILE: DIR + '/a.txt', PLATFORM_ADMIN_EMAIL: 'boss@x.com', PLATFORM_ADMIN_PASSWORD: 'bosspass123', STATIC_DIR: ROOT + '/frontend/dist' }, stdio: 'ignore' })
  await new Promise(r => setTimeout(r, 1500))
  const admin = (await call('POST', '/login', null, { email: 'boss@x.com', password: 'bosspass123' })).json.token
  const inst = (await call('POST', '/admin/institutions', admin, { type: 'university', name_ar: 'جامعة' })).json
  const subj = (await call('POST', '/admin/subjects', admin, { institution_id: inst.id, name_ar: 'مادة' })).json
  // 40 questions: 24 single (1 pt), 8 multi (2 pt), 5 true/false (1 pt), 3 short answers (4 pt)
  const Q = []
  for (let n = 1; n <= 40; n++) {
    const o = k => Array.from({ length: k }, (_, i) => `q${n}-opt${i}`)
    if (n <= 24) Q.push({ id: `q${n}`, type: 'single_choice', stem: `سؤال ${n}`, options: o(4), answer: 'ABCD'[n % 4], score: 1 })
    else if (n <= 32) Q.push({ id: `q${n}`, type: 'multi_choice', stem: `سؤال ${n}`, options: o(5), answer: 'AC', score: 2 })
    else if (n <= 37) Q.push({ id: `q${n}`, type: 'true_false', stem: `سؤال ${n}`, options: ['صحيح', 'خطأ'], answer: 'A', score: 1 })
    else Q.push({ id: `q${n}`, type: 'short_answer', stem: `سؤال ${n}`, answer: 'مرجع', score: 4 })
  }
  const exam = (await call('POST', '/admin/exams', admin, { subject_id: subj.id, title: 'امتحان الحمل', questions: Q, status: 'published', duration_min: 60, max_attempts: 1, shuffle_questions: true, shuffle_options: true, pass_mark: 50 })).json
  console.log(`server RSS at start: ${rssMb()} MB`)
  console.log(`exam ${exam.id}: ${exam.question_count} questions, ${exam.total_points} points; ${N} students\n`)

  // accounts (each IP may register 10/hour: spread over synthetic client addresses)
  const ids = Array.from({ length: N }, (_, i) => i)
  const reg = await pool(ids, 24, i => call('POST', '/register', null, { email: `st${i}@x.com`, password: 'password123', full_name: `طالب ${i}`, role: 'student', institution_type: 'university', consent: true }, `10.9.${Math.floor(i / 8)}.${i % 8}`))
  stats('register', reg)
  const logins = await pool(ids, 24, i => call('POST', '/login', null, { email: `st${i}@x.com`, password: 'password123' }, `10.8.${Math.floor(i / 8)}.${i % 8}`))
  stats('login', logins)
  const tok = logins.map(l => l.json.token)
  const enr = await pool(ids, 24, i => call('POST', '/enrollments', tok[i], { subject_id: subj.id }))
  stats('enroll', enr)

  console.log(`server RSS after creating ${N} accounts (argon2 hashing): ${rssMb()} MB`)
  // ── the burst: everybody presses Start at the same moment
  const starts = await Promise.all(ids.map(i => call('POST', `/assessments/${exam.id}/start`, tok[i], {})))
  const badStart = stats('START burst (all at once)', starts)
  const S = starts.map(s => s.json)
  ok('every student got an attempt with all 40 questions', badStart === 0 && S.every(s => s.questions && s.questions.length === 40))
  ok('no response leaks an answer, reference text or analysis', starts.every(s => !/"answer"|analysis|مرجع/.test(JSON.stringify(s.json))))
  const sigs = new Set(S.map(s => s.questions.map(q => q.id).join(',')))
  ok(`question orders are distinct (${sigs.size} distinct layouts among ${N})`, sigs.size >= N * 0.98)
  const optSigs = new Set(S.map(s => s.questions.map(q => q.options.join('|')).join(';')))
  ok(`full layouts (questions + options) are distinct (${optSigs.size})`, optSigs.size >= N * 0.99)
  ok('every question appears exactly once per attempt and true/false options keep their order', S.every(s => new Set(s.questions.map(q => q.id)).size === 40 && s.questions.filter(q => q.type === 'true_false').every(q => q.options.join() === 'صحيح,خطأ')))

  // what each student answers: the first k objective questions (in ORIGINAL order) right, by option text; others blank
  const originalById = Object.fromEntries(Q.map(q => [q.id, q]))
  const plan = ids.map(i => { const k = i % 31; const right = Q.filter(q => q.type !== 'short_answer').slice(0, k); return { right, expected: right.reduce((a, q) => a + q.score, 0) } })
  const answersFor = i => { const a = {}; for (const q of plan[i].right) { const shown = S[i].questions.find(x => x.id === q.id); if (q.type === 'true_false') a[q.id] = 'A'; else { const want = q.answer.split('').map(c => q.options['ABCDE'.indexOf(c)]); a[q.id] = shown.options.map((t, k) => want.includes(t) ? 'ABCDE'[k] : '').join('') } } return a }

  // ── autosave storm: every student saves 10 times in a row (the last one carries the real answers), all 300 streams at once
  const saves = (await Promise.all(ids.map(async i => { const rs = []; for (let r = 0; r < 10; r++) rs.push(await call('PUT', `/attempts/${S[i].attempt_id}/answers`, tok[i], { answers: r === 9 ? answersFor(i) : {} })); return rs }))).flat()
  stats('AUTOSAVE storm (300 streams x 10)', saves)
  console.log(`server RSS after the storm: ${rssMb()} MB`)
  // ── steady state: one save per student per second for 10 s (about 30x the real rate of one per 30 s)
  const steady = []; for (let sec = 0; sec < 10; sec++) { const t0 = Date.now(); steady.push(...await Promise.all(ids.map(i => call('PUT', `/attempts/${S[i].attempt_id}/answers`, tok[i], { answers: answersFor(i) })))); await new Promise(r => setTimeout(r, Math.max(0, 1000 - (Date.now() - t0)))) }
  const badSteady = stats('STEADY autosave 300/s for 10 s', steady)
  ok('steady-state saves never fail and stay fast (p95 < 500 ms)', badSteady === 0 && pct(steady.map(r => r.ms), .95) < 500)
  const events = await Promise.all(ids.map(i => call('POST', `/attempts/${S[i].attempt_id}/events`, tok[i], { type: 'tab_leave' })))
  stats('tab-leave events', events)
  // ── everybody reloads: resume must return the same layout and the last save
  const resumes = await Promise.all(ids.map(i => call('POST', `/assessments/${exam.id}/start`, tok[i], {})))
  stats('RESUME burst', resumes)
  ok('resume: same layout, same saved answers for all students', resumes.every((r, i) => r.json.resumed && r.json.questions.map(q => q.id).join() === S[i].questions.map(q => q.id).join() && norm(r.json.saved_answers) === norm(answersFor(i))))
  // ── submit wave
  const subs = await Promise.all(ids.map(i => call('POST', `/attempts/${S[i].attempt_id}/submit`, tok[i], { answers: answersFor(i) })))
  const badSub = stats('SUBMIT burst', subs)
  ok('every submission succeeded', badSub === 0)
  const wrong = subs.map((r, i) => [i, r.json.score, plan[i].expected]).filter(([, got, exp]) => got !== exp)
  ok(`every score is exactly right despite 300 different shuffles (${N - wrong.length}/${N})`, wrong.length === 0)
  if (wrong.length) console.log('   first mismatches:', wrong.slice(0, 5))
  const sum = await call('GET', `/assessments/${exam.id}/results`, admin)
  stats('results page (admin)', [sum])
  ok('admin summary counts all submissions, passers and tab leaves', sum.json.submitted === N && sum.json.attempts.length === N && sum.json.attempts.every(a => a.tab_leaves === 1))
  const passExpected = plan.filter(p => p.expected / exam.total_points * 100 + 1e-9 >= 50).length
  console.log(`   passers per server ${sum.json.passed}, expected ${passExpected} (total points ${exam.total_points})`)
  ok('pass count matches the independent calculation', sum.json.passed === passExpected)

  console.log(`\nserver RSS at the end: ${rssMb()} MB`)
  const integrity = execFileSync('python3', ['-I', '-c', `import sqlite3;c=sqlite3.connect('${DIR}/p.db');print(c.execute("pragma integrity_check").fetchone()[0], c.execute("select count(*) from attempts where status='submitted'").fetchone()[0])`]).toString().trim()
  ok(`database integrity after the storm: "${integrity}"`, integrity === `ok ${N}`)
  proc.kill(); console.log(failures ? `\n${failures} FAILED` : '\nALL PASSED'); process.exit(failures ? 1 : 0)
})().catch(e => { console.error(e); try { proc.kill() } catch {} process.exit(2) })
