import {
  FIRST_PAGE, MAX_QUERY, foldText, groupsFor, hasMoreHits, highlightParts, mergeHits, normalizeQuery, queryValid, safeHitLink, totalCount,
} from './searchGroups.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const parts = (t: string, q: string) => highlightParts(t, q).map((p) => (p.match ? `[${p.text}]` : p.text)).join('')

// --- the query ------------------------------------------------------------------------------------------------------------
eq(queryValid(''), false, 'empty')
eq(queryValid(' a '), false, 'one character')
eq(queryValid('ab'), true, 'two characters')
eq(queryValid('  رياضيات  '), true, 'Arabic word')
eq(queryValid('😀'), false, 'one astral character is one character')
eq(queryValid('😀😀'), true, 'two astral characters')
eq(normalizeQuery('  a   b \n c '), 'a b c', 'white space collapsed')
eq(normalizeQuery('x'.repeat(100)).length, MAX_QUERY, 'capped at the server limit')
eq(FIRST_PAGE, 8, 'first page size')

// --- groups ---------------------------------------------------------------------------------------------------------------
eq(groupsFor('student'), ['subjects', 'teachers', 'courses', 'posts', 'live', 'exams'], 'students see exams')
eq(groupsFor('teacher'), ['subjects', 'teachers', 'courses', 'posts', 'live'], 'teachers do not')
eq(groupsFor('admin').includes('exams'), false, 'admins do not')
eq(groupsFor(null).includes('exams'), false, 'unknown role does not')

// --- folding (mirrors the server) -----------------------------------------------------------------------------------------
eq(foldText('أحمد'), 'احمد', 'hamza above')
eq(foldText('إسلام'), 'اسلام', 'hamza below')
eq(foldText('آمن'), 'امن', 'madda')
eq(foldText('ٱلله'), 'الله', 'wasla')
eq(foldText('موسى'), 'موسي', 'alef maqsura')
eq(foldText('كِتَابٌ'), 'كتاب', 'tashkeel removed')
eq(foldText('مـــدرسة'), 'مدرسة', 'tatweel removed')
eq(foldText('Math'), 'math', 'lower case')
eq(foldText('مدرسة'), 'مدرسة', 'ta marbuta is NOT folded (the server keeps it)')
eq(foldText('هدف'), 'هدف', 'ha is not folded')

// --- highlighting ---------------------------------------------------------------------------------------------------------
eq(parts('Linear Algebra', 'algebra'), 'Linear [Algebra]', 'case-insensitive')
eq(parts('Algebra and algebra', 'ALGEBRA'), '[Algebra] and [algebra]', 'every occurrence')
eq(parts('Nothing here', 'zzz'), 'Nothing here', 'no match: the text as it is')
eq(parts('احمد في المدرسة', 'أحمد'), '[احمد] في المدرسة', 'hamza-folded query finds the bare alef')
eq(parts('أحمد في المدرسة', 'احمد'), '[أحمد] في المدرسة', 'and the other way round - the ORIGINAL text is shown')
eq(parts('كِتَابُ الرياضيات', 'كتاب'), '[كِتَابُ] الرياضيات', 'tashkeel inside the match stays in the highlighted piece')
eq(parts('مـــدرسة الأمل', 'مدرسة'), '[مـــدرسة] الأمل', 'tatweel inside the match')
eq(parts('موسى', 'موسي'), '[موسى]', 'alef maqsura vs ya')
eq(parts('Algebra', 'a b'), 'Algebra', 'a phrase that is not in the text and whose words are too short to try')
eq(parts('Algebra', 'alg ebra'), '[Alg][ebra]', 'words of two or more characters are tried one by one')
eq(parts('Linear equations and algebra', 'algebra linear'), '[Linear] equations and [algebra]', 'phrase missing: each word is tried')
eq(parts('abc', ''), 'abc', 'empty query')
eq(parts('', 'abc'), '', 'empty text')
eq(parts('😀 smile 😀', 'smile'), '😀 [smile] 😀', 'astral characters before the match keep their indexes')
eq(parts('a😀b', '😀'), 'a[😀]b', 'an astral match')
eq(parts('<b>bold</b> & more', 'bold'), '<b>[bold]</b> & more', 'markup is just text')
// the pieces always put the original back together
for (const [t, q] of [['كِتَابُ الرياضيات والعلوم', 'الرياضيات'], ['Linear Algebra', 'a'], ['aaa', 'aa'], ['أأأ', 'ا'], ['x😀y😀z', 'y']] as const) {
  eq(highlightParts(t, q).map((p) => p.text).join(''), t, `concatenation of "${t}" / "${q}"`)
}
// a one-letter query in a long text cannot make thousands of pieces
eq(highlightParts('a'.repeat(5000), 'aa').length <= 41, true, 'bounded number of pieces')

// --- paging ---------------------------------------------------------------------------------------------------------------
eq(mergeHits([{ id: 'a' }, { id: 'b' }], [{ id: 'b' }, { id: 'c' }]).map((h) => h.id), ['a', 'b', 'c'], 'appended without duplicates')
eq(mergeHits([], [{ id: 'x' }]).length, 1, 'into an empty list')
eq(hasMoreHits(20, 8), true, 'more to show')
eq(hasMoreHits(8, 8), false, 'all shown')
eq(hasMoreHits(undefined, 3), false, 'unknown count')
eq(totalCount({ subjects: 2, teachers: 1, exams: 4 }, ['subjects', 'teachers']), 3, 'only the groups the role sees')
eq(totalCount(null, ['subjects']), 0, 'no counts')

// --- links ----------------------------------------------------------------------------------------------------------------
eq(safeHitLink('/platform/courses/abc'), '/platform/courses/abc', 'in-app path')
eq(safeHitLink('javascript:alert(1)'), null, 'not a path')
eq(safeHitLink('//evil.example/platform/x'), null, 'protocol-relative')
eq(safeHitLink('/platform/courses/abc?lesson=l1'), '/platform/courses/abc?lesson=l1', 'a plain query is fine')
eq(safeHitLink('/platform/../x'), null, 'no dot segments')
console.log('search groups tests ok')
