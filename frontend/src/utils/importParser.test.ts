import { Difficulty } from '@exameow/shared'
import * as XLSX from 'xlsx'
import { analyzeCSV, analyzeExcel, parseWithMapping } from './importParser.ts'

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`)
}

const labels: Array<[string, Difficulty | undefined]> = [
  ['简单', Difficulty.Easy],
  ['easy', Difficulty.Easy],
  ['中等', Difficulty.Medium],
  ['medium', Difficulty.Medium],
  ['困难', Difficulty.Hard],
  ['hard', Difficulty.Hard],
  ['', undefined],
  ['unknown', undefined],
]

for (const [label, expected] of labels) {
  const analysis = analyzeCSV(`题干,答案,难度\nQ,A,${label}`)
  if (!analysis) throw new Error('expected CSV analysis')
  assertEqual(parseWithMapping(analysis, analysis.mapping, 'test')[0]?.difficulty, expected, `difficulty ${label}`)
}

for (const header of ['難易度', '難易度']) {
  const analysis = analyzeCSV(`题干,答案,${header}\nQ,A,hard`)
  if (!analysis) throw new Error(`expected CSV analysis for ${header}`)
  assertEqual(parseWithMapping(analysis, analysis.mapping, 'test')[0]?.difficulty, Difficulty.Hard, `header ${header}`)
}

const headerlessCanonical = analyzeCSV('Q,single_choice,A,B,C,D,E,F,G,H,A,,Physics,Chapter 1,hard')
if (!headerlessCanonical) throw new Error('expected headerless canonical CSV analysis')
assertEqual(headerlessCanonical.hasHeader, false, 'headerless canonical format')
assertEqual(parseWithMapping(headerlessCanonical, headerlessCanonical.mapping, 'test')[0]?.difficulty, Difficulty.Hard, 'headerless canonical difficulty')

const multiRowCanonical = analyzeCSV([
  'First question,single_choice,A,B,C,D,E,F,G,H,A,,Physics,Chapter 1,hard',
  'Second question,multi_choice,A,B,C,D,E,F,G,H,A,B,Physics,Chapter 2,medium',
].join('\n'))
if (!multiRowCanonical) throw new Error('expected multi-row headerless canonical CSV analysis')
assertEqual(multiRowCanonical.hasHeader, false, 'multi-row headerless canonical format')
const multiRowQuestions = parseWithMapping(multiRowCanonical, multiRowCanonical.mapping, 'test')
assertEqual(multiRowQuestions.length, 2, 'multi-row headerless canonical retains first row')
assertEqual(multiRowQuestions[0]?.stem, 'First question', 'multi-row headerless canonical first stem')
assertEqual(multiRowQuestions[0]?.difficulty, Difficulty.Hard, 'multi-row headerless canonical difficulty')
assertEqual(multiRowQuestions[1]?.difficulty, Difficulty.Medium, 'multi-row headerless canonical second difficulty')

const singleRowCanonical = analyzeCSV('Only question,single_choice,A,B,C,D,E,F,G,H,A,,Physics,Chapter 3,easy')
if (!singleRowCanonical) throw new Error('expected single-row headerless canonical CSV analysis')
assertEqual(singleRowCanonical.hasHeader, false, 'single-row headerless canonical format')
const singleRowQuestions = parseWithMapping(singleRowCanonical, singleRowCanonical.mapping, 'test')
assertEqual(singleRowQuestions.length, 1, 'single-row headerless canonical retains row')
assertEqual(singleRowQuestions[0]?.difficulty, Difficulty.Easy, 'single-row headerless canonical difficulty')

const xlsxWorkbook = XLSX.utils.book_new()
const xlsxSheet = XLSX.utils.aoa_to_sheet([
  ['题干（必填）', '题型 （必填）', '选项 A', '选项 B', '选项 C', '选项 D', '选项E\n(勿删)', '选项F\n(勿删)', '选项G\n(勿删)', '选项H\n(勿删)', '正确答案\n（必填）', '解析\n（勿删）', '章节\n（勿删）', '难度'],
  ['Question', '单选题', 'A', 'B', 'C', 'D', '', '', '', '', 'A', 'Analysis', '1章', '适中'],
])
XLSX.utils.book_append_sheet(xlsxWorkbook, xlsxSheet, '试题内容')
const xlsxBuffer = XLSX.write(xlsxWorkbook, { type: 'array', bookType: 'xlsx' })
const xlsxTemplateWithoutSubject = analyzeExcel(xlsxBuffer)
if (!xlsxTemplateWithoutSubject) throw new Error('expected xlsx template-shaped analysis')
const xlsxTemplateQuestion = parseWithMapping(xlsxTemplateWithoutSubject, xlsxTemplateWithoutSubject.mapping, 'xlsx')[0]
assertEqual(xlsxTemplateQuestion?.subject, undefined, 'xlsx template without subject leaves subject empty')
assertEqual(xlsxTemplateQuestion?.chapter, '1章', 'xlsx template maps chapter by header')
assertEqual(xlsxTemplateQuestion?.difficulty, Difficulty.Medium, 'xlsx template maps 适中 difficulty by header')

for (const [csv, expected] of [
  ['题干,答案,章节\nQ,A, 第一章 ', '第一章'],
  ['题干,答案,章节\nQ,A,   ', undefined],
  ['题干,答案\nQ,A', undefined],
] as const) {
  const analysis = analyzeCSV(csv)!
  assertEqual(parseWithMapping(analysis, analysis.mapping, 'test')[0]?.chapter, expected, 'chapter import compatibility')
}

// ───────── Arabic spreadsheets (phase 2-1) ─────────
import { parseWithMappingDetailed } from './importParser.ts'

function same(actual: unknown, expected: unknown, message: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`)
}
function sheetBuffer(rows: unknown[][]): ArrayBuffer {
  const wb = XLSX.utils.book_new()
  XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet(rows), 'الأسئلة')
  return XLSX.write(wb, { type: 'array', bookType: 'xlsx' }) as ArrayBuffer
}

const ARABIC_HEADER = ['السؤال', 'النوع', 'الخيار أ', 'الخيار ب', 'الخيار ج', 'الخيار د', 'الإجابة', 'الشرح', 'الفصل', 'الصعوبة', 'الدرجة']
const arabic = analyzeExcel(sheetBuffer([
  ARABIC_HEADER,
  ['ما عاصمة اليمن؟', 'اختيار من متعدد (إجابة واحدة)', 'عدن', 'صنعاء', 'تعز', 'إب', 'ب', 'العاصمة صنعاء', 'الوحدة 1', 'سهل', '٢'],
  ['اختر الأعداد الأولية', 'اختيار من متعدد (أكثر من إجابة)', '2', '4', '5', '9', 'أ، ج', '', 'الوحدة 1', 'متوسط', '1.5'],
  [],
  ['الأرض كروية', 'صح / خطأ', '', '', '', '', 'صواب', '', '', 'صعب', ''],
  ['أكمل: عاصمة مصر _____', 'أكمل الفراغ', '', '', '', '', 'القاهرة|مصر', '', '', '', ''],
  ['اشرح _____ بإيجاز', 'إجابة قصيرة', '', '', '', '', 'نموذج الإجابة', '', '', '', '٣٫٥'],
  ['', 'اختيار من متعدد (إجابة واحدة)', 'أ', 'ب', '', '', 'أ', '', '', '', ''],
  ['بلا إجابة صحيحة بالحروف', 'اختيار من متعدد (إجابة واحدة)', 'القاهرة', 'الرياض', '', '', 'الرياض', '', '', '', ''],
]))!
assertEqual(arabic.hasHeader, true, 'Arabic header row is recognised (not treated as question 1)')
assertEqual(arabic.mapping.stem, 0, 'stem')
assertEqual(arabic.mapping.type, 1, 'type column is not claimed by the stem ("نوع" vs "السؤال")')
same(arabic.mapping.options, [2, 3, 4, 5], 'option columns in letter order')
assertEqual(arabic.mapping.answer, 6, 'answer')
assertEqual(arabic.mapping.analysis, 7, 'analysis')
assertEqual(arabic.mapping.chapter, 8, 'chapter')
assertEqual(arabic.mapping.difficulty, 9, 'difficulty')
assertEqual(arabic.mapping.score, 10, 'score')

const detailed = parseWithMappingDetailed(arabic, arabic.mapping, 'xlsx')
const q = detailed.items.map(i => i.question)
same(detailed.items.map(i => i.rowNumber), [2, 3, 5, 6, 7, 9], 'real sheet rows (row 4 is blank and row 8 has no stem)')
same(detailed.skipped, [{ rowNumber: 8, reason: 'empty_stem' }], 'a non-blank row without a question is reported, not silently dropped')
same([q[0]?.type, q[0]?.answer, q[0]?.difficulty, q[0]?.score, q[0]?.chapter], ['single_choice', 'B', Difficulty.Easy, 2, 'الوحدة 1'], 'single choice with Arabic-Indic score')
same([q[1]?.type, q[1]?.answer, q[1]?.difficulty, q[1]?.score], ['multi_choice', 'AC', Difficulty.Medium, 1.5], 'multi choice: "أ، ج" → AC')
same([q[2]?.type, q[2]?.answer, q[2]?.difficulty], ['true_false', 'صحيح', Difficulty.Hard], 'true/false: صواب → صحيح')
same([q[3]?.type, q[3]?.answer], ['fill_blank', 'القاهرة|مصر'], 'fill blank keeps alternatives')
same([q[4]?.type, q[4]?.score], ['short_answer', 3.5], 'an explicit short-answer label wins over "_____" in the stem; ٣٫٥ → 3.5')
same([q[5]?.type, q[5]?.answer], ['single_choice', 'B'], 'an answer equal to an option text becomes its letter')
assertEqual(parseWithMapping(arabic, arabic.mapping, 'xlsx').length, 6, 'parseWithMapping returns the same questions')
assertEqual(arabic.rowNumbers?.length, arabic.rows.length, 'one row number per data row')

// the old behaviour is unchanged for Chinese/English sheets: blank rows still vanish, row numbers are still real
const chinese = analyzeExcel(sheetBuffer([['题干', '答案'], ['Q1', 'A'], [], ['Q2', 'B']]))!
same(chinese.rowNumbers, [2, 4], 'Chinese sheet row numbers skip the blank row')
assertEqual(parseWithMapping(chinese, chinese.mapping, 'x').length, 2, 'Chinese sheet still parses')

// sheets that start below row 1 keep real numbers
const offset = analyzeExcel(sheetBuffer([[], [], ['السؤال', 'الإجابة'], ['س1', 'ج1']]))!
same(offset.rowNumbers, [4], 'rows are numbered from the sheet, not from the first used row')

// a CSV saved by Excel carries a BOM; headers are still recognised
const csv = analyzeCSV('﻿السؤال,الإجابة\nس1,ج1\nس2,ج2')!
assertEqual(csv.hasHeader, true, 'BOM before the first header')
same(csv.rowNumbers, [2, 3], 'CSV record numbers')
assertEqual(parseWithMapping(csv, csv.mapping, 'csv').length, 2, 'Arabic CSV parses')

// without a type column the type is inferred, including Arabic-letter answers
const noType = analyzeCSV('السؤال,الخيار أ,الخيار ب,الخيار ج,الإجابة\nس,1,2,3,أ\nس,1,2,3,أ ب')!
same(parseWithMapping(noType, noType.mapping, 'csv').map(x => [x.type, x.answer]), [['single_choice', 'A'], ['multi_choice', 'AB']], 'inferred choice types with Arabic letters')

// the exact-match rule: "نوع السؤال" is the type, never the stem
const tricky = analyzeCSV('نوع السؤال,السؤال,الإجابة\nاختيار من متعدد,س,أ')!
assertEqual(tricky.mapping.type, 0, 'نوع السؤال → type')
assertEqual(tricky.mapping.stem, 1, 'السؤال → stem')
