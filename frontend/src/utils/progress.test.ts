import { MIN_BAR_PERCENT, barCss, barHeight, barLabel, clampPercent, fmtPercent, orderSubjects, statsEmpty, subjectHasResults, trendDirection } from './progress.ts'

function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(clampPercent(120), 100, 'above 100')
eq(clampPercent(-5), 0, 'below 0')
eq(clampPercent(null), 0, 'null')
eq(clampPercent(Number.NaN), 0, 'NaN')
eq(fmtPercent(85), '85', 'whole number, no .0')
eq(fmtPercent(72.46), '72.5', 'one decimal')
eq(fmtPercent(72.04), '72', 'a tiny fraction vanishes')
eq(fmtPercent(null), null, 'no value')
eq(barHeight(50), 50, 'half')
eq(barHeight(33.33), 33.3, 'one decimal')
eq(barHeight(250), 100, 'clamped')
eq(barCss(0), `${MIN_BAR_PERCENT}%`, 'a 0 % result is a thin stub')
eq(barCss(80), '80%', 'the bar is as tall as the percent')
eq(barLabel({ exam_title: 'Algebra 1', percent: 85 }), 'Algebra 1: 85%', 'exam and percent')
eq(barLabel({ exam_title: 'Quiz', percent: 66.67 }), 'Quiz: 66.7%', 'rounded')

const pts = (...xs: number[]) => xs.map((percent) => ({ percent }))
eq(trendDirection(pts(50, 60)), 'none', 'two attempts are no trend')
eq(trendDirection(pts(50, 55, 70, 80)), 'up', 'later results are higher')
eq(trendDirection(pts(90, 85, 60, 50)), 'down', 'later results are lower')
eq(trendDirection(pts(70, 72, 71, 73)), 'steady', 'within five points')
eq(trendDirection(pts(60, 60, 65)), 'up', 'three attempts: the last compared with the first')
eq(trendDirection(pts(60, 99, 58)), 'steady', 'three attempts: the middle one does not count')
eq(trendDirection([]), 'none', 'nothing')

const sub = (subject_id: string, attempts: number, avg: number | null, name = subject_id) =>
  ({ subject_id, subject_name: name, exams: attempts, attempts, average_percent: avg, trend: [], weakest: [] })
eq(subjectHasResults(sub('a', 2, 80)), true, 'results')
eq(subjectHasResults(sub('a', 0, null)), false, 'no attempts')
eq(subjectHasResults(sub('a', 2, null)), false, 'attempts without a counted average')
eq(statsEmpty(null), false, 'not loaded')
eq(statsEmpty({ overall: { attempts: 0, exams: 0, average_percent: null }, subjects: [] }), true, 'nothing counted')
eq(statsEmpty({ overall: { attempts: 0, exams: 0, average_percent: null }, subjects: [sub('a', 0, null)] }), true, 'subjects without results')
eq(statsEmpty({ overall: { attempts: 2, exams: 1, average_percent: 80 }, subjects: [sub('a', 2, 80)] }), false, 'has results')
eq(orderSubjects([sub('c', 0, null, 'C'), sub('b', 1, 50, 'B'), sub('a', 3, 90, 'A')]).map((s) => s.subject_id), ['a', 'b', 'c'], 'results first, most attempts first')
console.log('progress tests ok')
