import { arabicChoiceAnswer, arabicDifficulty, arabicLettersToLatin, arabicTrueFalseAnswer, arabicTypeFromLabel, classifyArabicHeader, foldAr, normalizeDigits } from './arabicImport.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}

eq(foldAr('  الإِجَابَة‏: '), 'الاجابه', 'folding drops diacritics/spaces/colon and unifies ا/ه')
eq(foldAr('﻿السؤال'), 'السؤال', 'BOM dropped')
eq(normalizeDigits('٣٫٥'), '3.5', 'Arabic-Indic digits and decimal mark')
eq(normalizeDigits('۱۲'), '12', 'Persian digits')
eq(normalizeDigits('١٬٢٠٠'), '1200', 'thousands separator dropped')

// headers
const f = (h: string) => classifyArabicHeader(h)
eq(f('السؤال'), { field: 'stem' }, 'stem')
eq(f('نص السؤال'), { field: 'stem' }, 'stem (long)')
eq(f('نوع السؤال'), { field: 'type' }, 'type is not claimed by the stem')
eq(f('النوع'), { field: 'type' }, 'type')
eq(f('الإجابة الصحيحة'), { field: 'answer' }, 'answer')
eq(f('الشرح'), { field: 'analysis' }, 'analysis')
eq(f('الفصل'), { field: 'chapter' }, 'chapter')
eq(f('الصعوبة'), { field: 'difficulty' }, 'difficulty')
eq(f('مستوى الصعوبة'), { field: 'difficulty' }, 'difficulty (long, ى folded)')
eq(f('الدرجة'), { field: 'score' }, 'score')
eq(f('الخيارات'), { field: 'options' }, 'combined options')
eq(f('الخيار أ'), { field: 'option', index: 0 }, 'option أ')
eq(f('الخيار ب'), { field: 'option', index: 1 }, 'option ب')
eq(f('الخيار د'), { field: 'option', index: 3 }, 'option د')
eq(f('الخيار هـ'), { field: 'option', index: 4 }, 'option هـ')
eq(f('خيار 3'), { field: 'option', index: 2 }, 'numbered option')
eq(f('ج'), { field: 'option', index: 2 }, 'bare letter header')
eq(f('السؤال الأول'), null, 'no substring matching')
eq(f('ملاحظات'), null, 'unknown header')
eq(f(''), null, 'empty')

// question types
const t = arabicTypeFromLabel
eq(t('اختيار من متعدد (إجابة واحدة)'), 'single_choice', 'single')
eq(t('اختيار من متعدد'), 'single_choice', 'plain "multiple choice" is single answer')
eq(t('اختيار من متعدد (أكثر من إجابة)'), 'multi_choice', 'multi')
eq(t('إجابات متعددة'), 'multi_choice', 'multi 2')
eq(t('صح / خطأ'), 'true_false', 'true/false')
eq(t('صح أو خطأ'), 'true_false', 'true/false 2')
eq(t('صواب وخطأ'), 'true_false', 'true/false 3')
eq(t('أكمل الفراغ'), 'fill_blank', 'fill blank')
eq(t('إكمال الفراغات'), 'fill_blank', 'fill blank 2')
eq(t('إجابة قصيرة'), 'short_answer', 'short answer')
eq(t('مقالي'), 'short_answer', 'essay')
eq(t('النوع'), null, 'the header word itself is not a type')
eq(t('الإجابة الصحيحة'), null, 'an answer header is not a type')
eq(t(''), null, 'empty')

// difficulty
eq([arabicDifficulty('سهل'), arabicDifficulty('متوسط'), arabicDifficulty(' صعب '), arabicDifficulty('سهلة')], ['easy', 'medium', 'hard', 'easy'], 'difficulty words')
eq(arabicDifficulty('غير معروف'), undefined, 'unknown difficulty')

// answers
const l = arabicLettersToLatin
eq(l('أ'), 'A', 'single')
eq(l('ب'), 'B', 'ب')
eq(l('ج'), 'C', 'ج')
eq(l('د'), 'D', 'د')
eq(l('هـ'), 'E', 'هـ')
eq(l('أ، ج'), 'AC', 'arabic comma')
eq(l('ج,أ'), 'AC', 'sorted + latin comma')
eq(l('أج'), 'AC', 'run of letters')
eq(l('أ و ج'), 'AC', '"و" between letters is "and"')
eq(l('و'), 'F', 'a lone و is the letter F')
eq(l('(ب)'), 'B', 'parentheses')
eq(l('ب.'), 'B', 'trailing dot')
eq(l('ب. القاهرة'), 'B', 'letter then option text')
eq(l('(ج) الرياض'), 'C', 'parenthesised letter then text')
eq(l('أ - ب'), 'AB', 'two letters with a hyphen are not "letter + text"')
eq(l('القاهرة'), null, 'a word is not letters')
eq(l(''), null, 'empty')

eq(arabicChoiceAnswer('ب', ['a', 'b']), 'B', 'letters within the options')
eq(arabicChoiceAnswer('د', ['a', 'b']), null, 'letter beyond the options is not an answer (falls through to text)')
eq(arabicChoiceAnswer('الرياض', ['القاهرة', 'الرياض']), 'B', 'option text → its letter')
eq(arabicChoiceAnswer('الرِّياض', ['القاهرة', 'الرياض']), 'B', 'diacritics ignored')
eq(arabicChoiceAnswer('عمان', ['القاهرة', 'الرياض']), null, 'unknown text')
eq(arabicChoiceAnswer('A', ['a', 'b']), null, 'Latin letters are left to the normal normaliser')
eq(arabicChoiceAnswer('Cairo', ['Cairo', 'Riyadh']), 'A', 'Latin option text is matched too')
eq(arabicChoiceAnswer('', ['a', 'b']), null, 'empty')
eq(arabicTrueFalseAnswer('صواب'), 'صحيح', 'صواب')
eq(arabicTrueFalseAnswer('صح'), null, 'already understood by the exam normaliser')

console.log('arabic import tests ok')
