import { classifyVideoUrl, embedsInPage, isHttpsUrl, MAX_URL_CHARS } from './videoKind.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (a !== b) throw new Error(`${msg}: expected ${String(b)}, got ${String(a)}`)
}

const YT = 'dQw4w9WgXcQ' // 11 characters

// --- the same cases the server's parse_video tests cover -------------------------------------------------------------
eq(classifyVideoUrl(`https://www.youtube.com/watch?v=${YT}`), 'youtube', 'watch url')
eq(classifyVideoUrl(`https://youtube.com/watch?v=${YT}&t=10s`), 'youtube', 'watch url with extra params')
eq(classifyVideoUrl(`https://m.youtube.com/watch?v=${YT}`), 'youtube', 'mobile host')
eq(classifyVideoUrl(`https://www.m.youtube.com/watch?v=${YT}`), 'youtube', 'www. then m.')
eq(classifyVideoUrl(`https://youtu.be/${YT}`), 'youtube', 'short link')
eq(classifyVideoUrl(`https://youtube.com/embed/${YT}`), 'youtube', 'embed')
eq(classifyVideoUrl(`https://youtube.com/shorts/${YT}`), 'youtube', 'shorts')
eq(classifyVideoUrl(`https://youtube.com/live/${YT}`), 'youtube', 'live')
eq(classifyVideoUrl('https://youtube.com/watch?v=short'), 'link', 'wrong id length is only a link')
eq(classifyVideoUrl(`https://youtube.com/watch?v=${YT}x`), 'link', 'too long id is only a link')
eq(classifyVideoUrl('https://youtube.com/watch'), 'link', 'no id')
eq(classifyVideoUrl('https://youtube.com/playlist?list=PL123'), 'link', 'playlist is not embedded')
eq(classifyVideoUrl('https://youtube.com/watch?v=abc$%^&*()'), 'link', 'odd characters in the id')
eq(classifyVideoUrl('https://youtu.be/'), 'link', 'short link without id')
eq(classifyVideoUrl('https://notyoutube.com/watch?v=dQw4w9WgXcQ'), 'link', 'look-alike host')
eq(classifyVideoUrl('https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ'), 'link', 'host that only starts with youtube.com')
eq(classifyVideoUrl('https://vimeo.com/123456789'), 'vimeo', 'vimeo id')
eq(classifyVideoUrl('https://www.vimeo.com/123456789'), 'vimeo', 'vimeo with www')
eq(classifyVideoUrl('https://vimeo.com/channels/staffpicks'), 'link', 'vimeo non-numeric')
eq(classifyVideoUrl('https://vimeo.com/123456789012345678901'), 'link', 'vimeo id over 20 characters')
eq(classifyVideoUrl('https://example.com/clip.mp4'), 'file', 'mp4')
eq(classifyVideoUrl('https://example.com/clip.WEBM'), 'file', 'extension is case-insensitive')
eq(classifyVideoUrl('https://example.com/a/clip.ogg?token=1'), 'file', 'query does not hide the extension')
eq(classifyVideoUrl('https://example.com/clip.mp4.html'), 'link', 'extension must be last')
eq(classifyVideoUrl('https://example.com/page'), 'link', 'any other page is an external link')

// --- what is not a link at all (the server answers 400 invalid_url) ----------------------------------------------------
eq(classifyVideoUrl('http://example.com/clip.mp4'), null, 'http is refused')
eq(classifyVideoUrl('javascript:alert(1)'), null, 'script scheme')
eq(classifyVideoUrl('ftp://example.com/x'), null, 'other scheme')
eq(classifyVideoUrl(''), null, 'empty')
eq(classifyVideoUrl('   '), null, 'blank')
eq(classifyVideoUrl('not a url'), null, 'not a url')
eq(classifyVideoUrl('https://exa mple.com/x'), null, 'white space inside')
eq(classifyVideoUrl('https://example.com/a\tb'), null, 'tab inside')
eq(classifyVideoUrl('https://example.com/a\u0007b'), null, 'control character')
eq(classifyVideoUrl('https://user:pw@example.com/x'), null, 'credentials')
eq(classifyVideoUrl('https://user@example.com/x'), null, 'user name only')
eq(classifyVideoUrl('https://'), null, 'no host')
eq(classifyVideoUrl(`https://example.com/${'a'.repeat(MAX_URL_CHARS)}`), null, 'over the length limit')
eq(classifyVideoUrl(`https://example.com/${'a'.repeat(MAX_URL_CHARS - 'https://example.com/'.length)}`) !== null, true, 'exactly at the limit')
eq(classifyVideoUrl('  https://example.com/clip.mp4  '), 'file', 'surrounding spaces are trimmed like the server does')

// --- helpers ---------------------------------------------------------------------------------------------------------
eq(isHttpsUrl('https://example.com'), true, 'https ok')
eq(isHttpsUrl('http://example.com'), false, 'http not ok')
eq(embedsInPage('youtube'), true, 'youtube embeds')
eq(embedsInPage('vimeo'), true, 'vimeo embeds')
eq(embedsInPage('file'), true, 'file embeds')
eq(embedsInPage('link'), false, 'link opens outside')
eq(embedsInPage(null), false, 'invalid does not embed')

console.log('video kind tests ok')
