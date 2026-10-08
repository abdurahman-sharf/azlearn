import * as XLSX from 'xlsx'
import type { Question } from '@exameow/shared'
import { buildTemplate, decodeCsv, ImportFileError, MAX_FILE_BYTES, parseQuestionFile, planImport, TEMPLATE_FILE_NAME, TYPE_LABELS } from './examImport.ts'

// Plain-script style like the other tests in this folder; the runner (scripts/test-chapters.cjs) awaits `done`.
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}
function book(rows: unknown[][]): ArrayBuffer {
  const wb = XLSX.utils.book_new()
  XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet(rows), 'الأسئلة')
  return XLSX.write(wb, { type: 'array', bookType: 'xlsx' }) as ArrayBuffer
}
async function fails(p: Promise<unknown>, code: string, msg: string): Promise<void> {
  try {
    await p
  } catch (e) {
    eq(e instanceof ImportFileError ? e.code : String(e), code, msg)
    return
  }
  throw new Error(`${msg}: expected ${code}, nothing thrown`)
}
const H = ['السؤال', 'النوع', 'الخيار أ', 'الخيار ب', 'الخيار ج', 'الإجابة', 'الشرح', 'الصعوبة', 'الدرجة']
const existing = (stem: string) => ({ id: 'x', type: 'short_answer', stem, options: [], answer: 'ج', analysis: '' }) as Question

export const done = (async () => {
  // the downloadable template is itself a valid import: five examples, one per type, all passing every check
  const template = await buildTemplate()
  const wb = XLSX.read(new Uint8Array(template), { type: 'array' })
  eq(wb.SheetNames, ['الأسئلة', 'تعليمات'], 'template sheets')
  ok(wb.Workbook?.Views?.[0]?.RTL, 'template opens right-to-left')
  ok(/^[\x20-\x7e]+$/.test(TEMPLATE_FILE_NAME), 'ASCII file name')
  const parsedTemplate = await parseQuestionFile('t.xlsx', template)
  eq(parsedTemplate.rows.map((r) => r.question.type), ['single_choice', 'multi_choice', 'true_false', 'fill_blank', 'short_answer'], 'template row types')
  const plan = planImport(parsedTemplate, [], 200, true)
  eq(plan.problems, [], 'the template has no problems')
  eq(plan.good.map((g) => g.question.answer), ['B', 'AC', 'A', 'القاهرة', 'قوة تجذب الأجسام بعضها إلى بعض.'], 'normalised answers (letters / A for صح)')
  eq(plan.good.map((g) => g.question.score), [1, 2, 1, 1, 3], 'scores from the sheet')
  eq(plan.good.every((g) => g.question.id === ''), true, 'ids are assigned when the editor adopts them')
  eq(plan.good[3]!.question.options, [], 'fill-blank rows carry no options')

  // real sheet rows in the report; each kind of problem is named
  const rows = book([
    H,
    ['س1 سؤال جيد', TYPE_LABELS.single, 'أ1', 'ب1', 'ج1', 'ب', 'شرح', 'سهل', '١'],
    [],
    ['', TYPE_LABELS.single, 'أ', 'ب', '', 'أ', '', '', ''],
    ['خيار واحد فقط', TYPE_LABELS.single, 'فقط', '', '', 'أ', '', '', ''],
    ['إجابة خارج الخيارات', TYPE_LABELS.single, 'أ1', 'ب1', '', 'ج', '', '', ''],
    ['شرح طويل جدًا', TYPE_LABELS.short, '', '', '', 'نموذج', 'ش'.repeat(6001), '', ''],
    ['درجة عالية', TYPE_LABELS.short, '', '', '', 'نموذج', '', '', '500'],
    ['س1 سؤال جيد', TYPE_LABELS.short, '', '', '', 'مكرر داخل الملف', '', '', ''],
    ['سؤال موجود في الامتحان', TYPE_LABELS.short, '', '', '', 'ج', '', '', ''],
    ['سؤال جيد آخر', TYPE_LABELS.trueFalse, '', '', '', 'خطأ', '', 'صعب', '2'],
  ])
  const parsed = await parseQuestionFile('q.xlsx', rows)
  eq(parsed.emptyRows, [4], 'a row with no question text is reported at its sheet row')
  const p = planImport(parsed, [existing('سؤال  موجود في الامتحان')], 200, true)
  eq(p.problems.map((x) => [x.rowNumber, x.issue]), [[4, 'empty_stem'], [5, 'options'], [6, 'answer'], [7, 'length'], [8, 'score'], [9, 'duplicate_file'], [10, 'duplicate_exam']], 'every problem with its real row')
  eq(p.good.map((g) => [g.rowNumber, g.question.stem, g.question.answer]), [[2, 'س1 سؤال جيد', 'B'], [11, 'سؤال جيد آخر', 'B']], 'good rows keep their order and row numbers')
  eq(p.good[0]!.question.score, 1, 'Arabic-Indic score ١ → 1')
  eq(planImport(parsed, [existing('سؤال موجود في الامتحان')], 200, false).good.length, 4, 'with duplicate-skipping off, duplicates are kept (and the other checks still apply)')

  // the exam holds at most 200 questions
  eq(planImport(parsedTemplate, [], 3, true).overflow, 2, '5 good questions, room for 3')
  eq(planImport(parsedTemplate, [], 5, true).overflow, 0, 'exactly fits')
  eq(planImport(parsedTemplate, [], 0, true).overflow, 5, 'no room')

  // files that cannot be used
  await fails(parseQuestionFile('x.pdf', new ArrayBuffer(10)), 'bad_type', 'unsupported extension')
  await fails(parseQuestionFile('big.xlsx', new ArrayBuffer(MAX_FILE_BYTES + 1)), 'too_large', 'over 5 MB')
  await fails(parseQuestionFile('e.xlsx', book([[]])), 'empty', 'empty sheet')
  await fails(parseQuestionFile('h.xlsx', book([['السؤال', 'ملاحظات'], ['س', 'ج']])), 'missing_columns', 'no answer column')
  await fails(parseQuestionFile('m.xlsx', book([['الإجابة', 'الشرح'], ['ج', 'ش']])), 'missing_columns', 'no question column')
  await fails(parseQuestionFile('many.xlsx', book([['السؤال', 'الإجابة'], ...Array.from({ length: 1001 }, (_, i) => [`س${i}`, 'ج'])])), 'too_many_rows', 'over 1000 rows')
  // SheetJS reads arbitrary bytes as one text cell instead of throwing, so garbage ends up as "no recognisable columns"
  await fails(parseQuestionFile('bad.xlsx', new Uint8Array([1, 2, 3, 4]).buffer), 'missing_columns', 'not a spreadsheet')

  // CSV: UTF-8 (with BOM) and the Windows-1256 files Excel for Arabic Windows writes
  const utf8 = new TextEncoder().encode('﻿السؤال,الإجابة\nما هي الحاسوب,جهاز\n').buffer as ArrayBuffer
  eq((await parseQuestionFile('q.csv', utf8)).rows.map((r) => [r.rowNumber, r.question.stem]), [[2, 'ما هي الحاسوب']], 'UTF-8 CSV with BOM')
  eq(decodeCsv(new Uint8Array([0xd3, 0x2c, 0xcc]).buffer as ArrayBuffer), 'س,ج', 'Windows-1256 fallback')
  eq(decodeCsv(new TextEncoder().encode('س,ج').buffer as ArrayBuffer), 'س,ج', 'UTF-8 stays UTF-8')

  console.log('exam import tests ok')
})()
