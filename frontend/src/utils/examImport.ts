/**
 * Importing exam questions from a spreadsheet (phase 2-1): read a .xlsx/.xls/.csv file, turn every row into a question
 * (Arabic or Chinese/English headers — see `importParser`/`arabicImport`), check each row against the same rules the
 * exam editor and the server apply, and report problems by their real sheet row so the admin can fix the file.
 */
import type { Question } from '@exameow/shared'
import { adoptQuestions, normText, validateQuestion, type Problem } from './examBuilder'

export const MAX_FILE_BYTES = 5 * 1024 * 1024
export const MAX_ROWS = 1000
/** Questions per exam (the server's limit). */
export const MAX_EXAM_QUESTIONS = 200

export type FileErrorCode = 'too_large' | 'bad_type' | 'empty' | 'too_many_rows' | 'missing_columns' | 'unreadable'
export class ImportFileError extends Error {
  constructor(readonly code: FileErrorCode) {
    super(code)
  }
}

export interface RawRow {
  rowNumber: number
  question: Question
}
export interface ParsedFile {
  rows: RawRow[]
  /** non-blank rows without any question text */
  emptyRows: number[]
  /** data rows in the sheet (blank ones excluded) */
  dataRows: number
}

/** UTF-8 first; Excel for Arabic Windows saves "CSV" as Windows-1256, so fall back to that on invalid UTF-8. */
export function decodeCsv(bytes: ArrayBuffer): string {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    return new TextDecoder('windows-1256').decode(bytes)
  }
}

/** Reads a spreadsheet file's bytes into question rows. Throws `ImportFileError` with a code the UI can explain. */
export async function parseQuestionFile(name: string, bytes: ArrayBuffer): Promise<ParsedFile> {
  if (bytes.byteLength > MAX_FILE_BYTES) throw new ImportFileError('too_large')
  const lower = name.toLowerCase()
  const isExcel = /\.(xlsx|xls)$/.test(lower)
  if (!isExcel && !lower.endsWith('.csv')) throw new ImportFileError('bad_type')
  // loaded on demand: the spreadsheet library stays out of the editor's main chunk
  const parser = await import('./importParser')
  let analysis
  try {
    analysis = isExcel ? parser.analyzeExcel(bytes) : parser.analyzeCSV(decodeCsv(bytes))
  } catch {
    throw new ImportFileError('unreadable')
  }
  if (!analysis || !analysis.rows.length) throw new ImportFileError('empty')
  if (analysis.rows.length > MAX_ROWS) throw new ImportFileError('too_many_rows')
  const { mapping } = analysis
  if (mapping.stem === null || mapping.answer === null) throw new ImportFileError('missing_columns')
  const { items, skipped } = parser.parseWithMappingDetailed(analysis, mapping, isExcel ? 'xlsx' : 'csv')
  return {
    rows: items.map((i) => ({ rowNumber: i.rowNumber, question: i.question })),
    emptyRows: skipped.map((s) => s.rowNumber),
    dataRows: items.length + skipped.length,
  }
}

export type RowIssue = Problem | 'empty_stem' | 'duplicate_file' | 'duplicate_exam'
export interface RowProblem {
  rowNumber: number
  issue: RowIssue
  /** the start of the question text, to recognise the row */
  stem: string
}
export interface ImportPlan {
  /** questions that passed every check, normalised (answers as letters, true/false options) and ready to be added */
  good: { rowNumber: number; question: Question }[]
  problems: RowProblem[]
  /** how many good questions do not fit under the exam's question limit */
  overflow: number
}

/** Duplicate key: the Arabic-folded stem with runs of whitespace collapsed. */
const dupKey = (s: string) => normText(s).replace(/\s+/g, ' ')
const snippet = (s: string) => (s.length > 60 ? `${s.slice(0, 60)}…` : s)

/**
 * Checks every parsed row. `existing` are the questions already in the exam (duplicates are matched on the folded
 * stem); `room` is how many more questions the exam can take. Problems come out sorted by row.
 */
export function planImport(parsed: ParsedFile, existing: Question[], room: number, skipDuplicates: boolean): ImportPlan {
  const problems: RowProblem[] = parsed.emptyRows.map((rowNumber) => ({ rowNumber, issue: 'empty_stem' as const, stem: '' }))
  const good: ImportPlan['good'] = []
  const inExam = new Set(existing.map((q) => dupKey(q.stem)))
  const inFile = new Set<string>()
  for (const { rowNumber, question } of parsed.rows) {
    // the same normalisation the editor applies when it adopts a question, so what is checked is what gets added
    const [adopted] = adoptQuestions([question], () => 'import')
    if (!adopted) continue
    const bad = validateQuestion(adopted)
    if (bad) {
      problems.push({ rowNumber, issue: bad, stem: snippet(adopted.stem) })
      continue
    }
    const key = dupKey(adopted.stem)
    if (skipDuplicates && inExam.has(key)) {
      problems.push({ rowNumber, issue: 'duplicate_exam', stem: snippet(adopted.stem) })
      continue
    }
    if (skipDuplicates && inFile.has(key)) {
      problems.push({ rowNumber, issue: 'duplicate_file', stem: snippet(adopted.stem) })
      continue
    }
    inFile.add(key)
    const { id: _id, src: _src, ...rest } = adopted
    good.push({ rowNumber, question: { id: '', ...rest } as Question })
  }
  problems.sort((a, b) => a.rowNumber - b.rowNumber)
  return { good, problems, overflow: Math.max(0, good.length - Math.max(0, room)) }
}

// ───────── template ─────────

/** The labels the importer recognises, one per question type (kept distinct so single vs multi cannot be confused). */
export const TYPE_LABELS = {
  single: 'اختيار من متعدد (إجابة واحدة)',
  multi: 'اختيار من متعدد (أكثر من إجابة)',
  trueFalse: 'صح / خطأ',
  fill: 'أكمل الفراغ',
  short: 'إجابة قصيرة',
} as const

export const TEMPLATE_HEADER = ['السؤال', 'النوع', 'الخيار أ', 'الخيار ب', 'الخيار ج', 'الخيار د', 'الخيار هـ', 'الإجابة', 'الشرح', 'الفصل', 'الصعوبة', 'الدرجة']

export const TEMPLATE_ROWS: string[][] = [
  ['ما عاصمة الجمهورية اليمنية؟', TYPE_LABELS.single, 'عدن', 'صنعاء', 'تعز', 'إب', '', 'ب', 'صنعاء هي العاصمة.', 'الوحدة 1', 'سهل', '1'],
  ['أيٌّ مما يلي أعداد أولية؟', TYPE_LABELS.multi, '2', '4', '5', '9', '', 'أ، ج', '2 و5 عددان أوليان.', 'الوحدة 1', 'متوسط', '2'],
  ['الأرض كروية الشكل تقريبًا.', TYPE_LABELS.trueFalse, '', '', '', '', '', 'صح', '', 'الوحدة 2', 'سهل', '1'],
  ['عاصمة مصر هي _____.', TYPE_LABELS.fill, '', '', '', '', '', 'القاهرة', 'يمكن كتابة بدائل: القاهرة|مصر', 'الوحدة 2', 'سهل', '1'],
  ['اشرح مفهوم الجاذبية بإيجاز.', TYPE_LABELS.short, '', '', '', '', '', 'قوة تجذب الأجسام بعضها إلى بعض.', 'يصحّحها المعلم يدويًا.', 'الوحدة 3', 'صعب', '3'],
]

export const TEMPLATE_NOTES: string[][] = [
  ['العمود', 'الوصف', 'القيم المقبولة'],
  ['السؤال', 'نص السؤال (مطلوب)', ''],
  ['النوع', 'نوع السؤال', Object.values(TYPE_LABELS).join(' — ')],
  ['الخيار أ … هـ', 'خيارات الأسئلة الاختيارية (خياران على الأقل)', ''],
  ['الإجابة', 'اختيار: حرف أو أكثر (أ، ج) أو نص الخيار نفسه · صح/خطأ: صح أو خطأ · أكمل الفراغ: الإجابة أو بدائل مفصولة بـ | · إجابة قصيرة: نموذج الإجابة', ''],
  ['الشرح', 'اختياري', ''],
  ['الفصل', 'اختياري: الفصل أو الباب لتجميع الأسئلة', ''],
  ['الصعوبة', 'اختياري', 'سهل — متوسط — صعب'],
  ['الدرجة', 'اختياري (الافتراضي 1)', 'رقم من 0 إلى 100'],
]

/** Builds the downloadable Arabic template (right-to-left, one example per question type, plus an instructions sheet). */
export async function buildTemplate(): Promise<ArrayBuffer> {
  const XLSX = await import('xlsx')
  const wb = XLSX.utils.book_new()
  const questions = XLSX.utils.aoa_to_sheet([TEMPLATE_HEADER, ...TEMPLATE_ROWS])
  questions['!cols'] = [{ wch: 44 }, { wch: 34 }, ...Array.from({ length: 5 }, () => ({ wch: 16 })), { wch: 28 }, { wch: 28 }, { wch: 14 }, { wch: 10 }, { wch: 8 }]
  XLSX.utils.book_append_sheet(wb, questions, 'الأسئلة')
  const notes = XLSX.utils.aoa_to_sheet(TEMPLATE_NOTES)
  notes['!cols'] = [{ wch: 18 }, { wch: 70 }, { wch: 70 }]
  XLSX.utils.book_append_sheet(wb, notes, 'تعليمات')
  wb.Workbook = { Views: [{ RTL: true }] }
  return XLSX.write(wb, { type: 'array', bookType: 'xlsx' }) as ArrayBuffer
}

/** ASCII on purpose: Arabic file names turn into "download" in some browsers. */
export const TEMPLATE_FILE_NAME = 'azlearn-exam-questions-template.xlsx'

export async function downloadTemplate(): Promise<void> {
  const blob = new Blob([await buildTemplate()], { type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = TEMPLATE_FILE_NAME
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}
