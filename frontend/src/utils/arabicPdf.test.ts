import { assembleArabicPage, fixVisualOrder, hasArabic, tidyLine, toBaseLetters } from './arabicPdf.ts'
import { ARABIC_PDF_ITEMS } from './arabicPdfItems.fixture.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
function ok(cond: unknown, msg: string): void {
  if (!cond) throw new Error(msg)
}

// ── string algorithm (same cases as packages/core/src/parser/arabic.rs)
eq(fixVisualOrder('تﺎﻧﺎﻴﺒﻟا ﺪﻋاﻮﻗ ﻲﻓ ﺔﻣﺪﻘﻣ'), 'مقدمة في قواعد البيانات', 'a shaped visual line')
eq(fixVisualOrder('SQL ﻭ 2024 ﺔﻨﺳ'), 'سنة 2024 و SQL', 'numbers and Latin keep their order')
eq(fixVisualOrder('ﻲﻓ 3.14 ﻢﻗﺭ'), 'رقم 3.14 في', 'decimal numbers stay whole')
const logical = 'نظام إدارة قاعدة البيانات يتيح الإنشاء والتعديل والاستعلام. المفتاح الأساسي يميز كل سجل في الجدول.'
eq(fixVisualOrder(logical), logical, 'logical text is untouched')
for (const s of ['Plain English text.\nSecond line 123', '', '你好，世界']) eq(fixVisualOrder(s), s, 'non-Arabic passes through')
eq(toBaseLetters('ﻻ', false), 'لا', 'ligature in logical order')
eq(toBaseLetters('ﻻ', true), 'ال', 'ligature reversed for a visual line')
eq(tidyLine('ا لإنشاء  و التعديل'), 'الإنشاء و التعديل', 'repairs')
eq(tidyLine('إلكترونيا ً .'), 'إلكترونياً .', 'a vowel mark never stands alone')
eq(tidyLine('ت ُخ ز ن'), 'تُخ ز ن', 'a mark glued to the next letter still belongs to the previous one')
ok(hasArabic('abc ب') && !hasArabic('abc'), 'hasArabic')

// ── pdf.js items of a real Arabic PDF (one item per shaped glyph, as Chromium writes them)
const page = assembleArabicPage(ARABIC_PDF_ITEMS)
const lines = page.split('\n')
for (const expected of ['مقدمة في قواعد البيانات', 'قاعدة البيانات هي', 'نظام', 'المفتاح', 'القيم الفارغة']) ok(page.includes(expected), `missing ${expected} in:\n${page}`)
ok(lines[0] === 'مقدمة في قواعد البيانات', `the heading is the first line: ${lines[0]}`)
ok(!/[ﭐ-﷿ﹰ-﻿]/.test(page), 'no presentation forms left')
ok(!/ [ا] ل/.test(page), 'the article is not split')
ok(lines.length >= 4 && lines.length <= 8, `lines are rebuilt (${lines.length}): \n${page}`)

// numbers inside an Arabic line, as pdf.js reports them (LTR item between RTL items; positions left to right)
const it = (str: string, x: number, dir: string, w = 20): { str: string; dir: string; transform: number[]; width: number; height: number } => ({ str, dir, transform: [12, 0, 0, 12, x, 100], width: w, height: 12 })
eq(assembleArabicPage([it('سنة', 120, 'rtl'), it(' ', 110, 'ltr', 10), it('2024', 70, 'ltr', 40), it(' ', 60, 'ltr', 10), it('عام', 20, 'rtl', 40)]), 'سنة 2024 عام', 'item order is right to left, the number stays whole')
eq(assembleArabicPage([it('Hello', 10, 'ltr', 40), it(' ', 50, 'ltr', 10), it('world', 60, 'ltr', 40)]), 'Hello world', 'an English line reads left to right')
eq(assembleArabicPage([it('أول', 100, 'rtl'), { ...it('ثان', 100, 'rtl'), transform: [12, 0, 0, 12, 100, 60] }]), 'أول\nثان', 'lines are ordered top to bottom')
eq(assembleArabicPage([]), '', 'empty')

console.log('arabic pdf tests ok')
