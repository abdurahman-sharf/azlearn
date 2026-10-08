import { fixVisualOrder } from './arabicPdf'

export interface ParseResult {
  text: string
  fileName: string
}

export async function parseFile(
  fileData: ArrayBuffer,
  fileName: string
): Promise<string> {
  const ext = fileName.split('.').pop()?.toLowerCase() || 'txt'

  switch (ext) {
    case 'txt':
    case 'md':
    case 'json':
    case 'xml':
    case 'log':
      return decodeUtf8(fileData)
    case 'csv':
      return decodeUtf8(fileData)
    case 'docx':
      return parseDocx(fileData)
    case 'xlsx':
    case 'xlsm':
    case 'xls':
      return parseXlsx(fileData)
    case 'ods':
      return parseOds(fileData)
    case 'html':
    case 'htm':
      return parseHtml(fileData)
    case 'pdf':
      return parsePdf(fileData)
    case 'pptx':
      return parsePptx(fileData)
    case 'odt':
      return parseOdt(fileData)
    case 'epub':
      return parseEpub(fileData)
    default:
      return decodeUtf8(fileData)
  }
}

function decodeUtf8(data: ArrayBuffer): string {
  return new TextDecoder('utf-8', { fatal: false, ignoreBOM: true }).decode(data)
}

// --- HTML ---
function parseHtml(data: ArrayBuffer): string {
  const raw = decodeUtf8(data)
  return raw
    .replace(/<style[^>]*>[\s\S]*?<\/style>/gi, '')
    .replace(/<script[^>]*>[\s\S]*?<\/script>/gi, '')
    .replace(/<[^>]+>/g, '\n')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

function toUtf8(data: Uint8Array): string {
  return new TextDecoder('utf-8', { fatal: false, ignoreBOM: true }).decode(data)
}

// --- DOCX ---
async function parseDocx(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)
  console.log('[parser:docx] zip files:', Object.keys(zip).join(', '))
  console.log('[parser:docx] data size:', data.byteLength)

  // Try multiple possible paths for the document XML
  const docPaths = ['word/document.xml', 'Word/Document.xml', 'WORD/DOCUMENT.XML']
  let docXml: Uint8Array | undefined
  for (const p of docPaths) {
    docXml = zip[p]
    if (docXml) {
      console.log('[parser:docx] found document at:', p, 'size:', docXml.byteLength)
      break
    }
  }

  if (!docXml) {
    // Try case-insensitive search
    for (const key of Object.keys(zip)) {
      if (key.toLowerCase().endsWith('document.xml')) {
        docXml = zip[key]
        console.log('[parser:docx] found document via case-insensitive:', key, 'size:', docXml.byteLength)
        break
      }
    }
  }

  if (!docXml) {
    console.log('[parser:docx] word/document.xml not found in zip')
    return fallbackDecode(data, 'docx')
  }

  const xml = toUtf8(docXml)
  console.log('[parser:docx] xml first 300 chars:', xml.substring(0, 300))

  // Method 1: extract w:t within w:p containers
  let text = extractXmlText(xml, 'w:t', 'w:p').join('\n')
  console.log('[parser:docx] extractXmlText(w:t, w:p) result length:', text.length)

  // Method 2: fallback - try without container
  if (!text.trim()) {
    text = extractXmlText(xml, 'w:t').join('\n')
    console.log('[parser:docx] extractXmlText(w:t only) result length:', text.length)
  }

  // Method 3: fallback - strip all XML tags
  if (!text.trim()) {
    console.log('[parser:docx] XML extraction failed, falling back to strip-tags')
    text = xml.replace(/<[^>]+>/g, '\n').replace(/\n{3,}/g, '\n\n').trim()
  }

  console.log('[parser:docx] final text length:', text.length)
  return text || fallbackDecode(data, 'docx')
}

// --- XLSX ---
async function parseXlsx(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)

  // Parse shared strings
  const sstXml = zip['xl/sharedStrings.xml']
  const sharedStrings = sstXml ? extractXmlText(toUtf8(sstXml), 't', 'si') : []

  // Parse each sheet
  const sheetFiles = Object.keys(zip).filter(k => k.match(/xl\/worksheets\/sheet\d+\.xml/)).sort()
  const texts: string[] = []

  for (const sheetFile of sheetFiles) {
    const sheetXml = toUtf8(zip[sheetFile])

    // Try to get sheet name from workbook.xml
    const workbookXml = zip['xl/workbook.xml']
    let sheetName = sheetFile
    if (workbookXml) {
      const wbStr = toUtf8(workbookXml)
      const sheetMatch = wbStr.match(/<sheet[^>]*\/>/g)
      if (sheetMatch) {
        const idx = sheetFiles.indexOf(sheetFile)
        if (idx >= 0 && sheetMatch[idx]) {
          const nameMatch = sheetMatch[idx].match(/name="([^"]*)"/)
          if (nameMatch) sheetName = nameMatch[1]
        }
      }
    }

    // Extract cell values
    const cells: string[] = []
    // v elements contain values (either inline or shared string index)
    const vRegex = /<c[^>]*>([\s\S]*?)<\/c>/g
    let vMatch
    while ((vMatch = vRegex.exec(sheetXml)) !== null) {
      const cContent = vMatch[1]
      const tMatch = cContent.match(/t="([^"]*)"/)
      const t = tMatch ? tMatch[1] : null

      const valMatch = cContent.match(/<v>([^<]*)<\/v>/)
      if (!valMatch) continue

      const val = valMatch[1]

      if (t === 's' && sharedStrings[parseInt(val)]) {
        cells.push(sharedStrings[parseInt(val)])
      } else if (t === 'inlineStr') {
        const isMatch = cContent.match(/<t[^>]*>([^<]*)<\/t>/)
        if (isMatch) cells.push(isMatch[1])
      } else {
        cells.push(val)
      }
    }

    if (cells.length) {
      texts.push(`[${sheetName}]\n${cells.join('\t')}`)
    }
  }

  return texts.join('\n\n') || fallbackDecode(data, 'xlsx')
}

// --- ODS ---
async function parseOds(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)
  const contentXml = zip['content.xml']
  if (!contentXml) return fallbackDecode(data, 'ods')

  const xml = toUtf8(contentXml)
  return extractXmlText(xml, 'text:p').join('\n')
}

// --- PDF ---
// macOS Quartz 生成的 PDF 常把汉字映射到 Kangxi 部首/CJK 兼容字符码点，按 NFKC 归一化
function normalizeCompatChars(s: string): string {
  return s.replace(/[\u2E80-\u2EFF\u2F00-\u2FDF\uF900-\uFAFF]/g, (ch) => ch.normalize('NFKC'))
}

async function parsePdf(data: ArrayBuffer): Promise<string> {
  const raw = new TextDecoder('latin1', { fatal: false, ignoreBOM: true }).decode(data)
  const rawBytes = new Uint8Array(data)

  // Build CID->Unicode mapping from font ToUnicode CMap streams
  const cidMap = await buildCidMap(raw, rawBytes)

  // Decompress all FlateDecoded content streams
  const contents = await decompressPdfContentStreams(raw, rawBytes)

  // Extract text from all content (raw + decompressed)
  const allText: string[] = []
  for (const content of contents) {
    extractPdfText(content, allText)
  }

  // Apply CMap to hex-encoded text and decode
  const decoded = allText.map(t => decodePdfText(t, cidMap)).filter(Boolean)
  const unique = [...new Set(decoded)]

  if (unique.length > 0) return normalizeCompatChars(fixVisualOrder(unique.join('\n')))
  return fallbackDecode(data, 'pdf')
}

async function buildCidMap(raw: string, rawBytes: Uint8Array): Promise<Map<string, string>> {
  const cidMap = new Map<string, string>()

  // Find font objects and their ToUnicode references
  // Pattern: N 0 obj ... /BaseFont/XXX /ToUnicode M 0 R ... endobj
  const fontRegex = /(\d+ \d+ obj[\s\S]*?\/BaseFont\/([^/]+)[\s\S]*?\/ToUnicode\s+(\d+ \d+ R)[\s\S]*?endobj)/g
  const fontToUnicodes: Record<string, string> = {} // fontName -> objRef

  let match
  while ((match = fontRegex.exec(raw)) !== null) {
    const fontName = match[2]
    const toUnicodeRef = match[3]
    const objNum = toUnicodeRef.split(' ')[0]
    fontToUnicodes[fontName] = objNum
  }

  // Parse each ToUnicode CMap stream (using regex on latin1 text)
  for (const [fontName, objNum] of Object.entries(fontToUnicodes)) {
    const cmapRegex = new RegExp(objNum + ' 0 obj[\\\\s\\\\S]*?/Filter\\\\s*(/FlateDecode)?[\\\\s\\\\S]*?stream\\\\r?\\\\n([\\\\s\\\\S]*?)endstream')
    const cmapMatch = raw.match(cmapRegex)
    if (!cmapMatch) continue

    let streamData = cmapMatch[2]
    const lenMatch = cmapMatch[0].match(/\/Length\s+(\d+)/)
    if (lenMatch) {
      const length = parseInt(lenMatch[1])
      if (streamData.length > length) streamData = streamData.substring(0, length)
    }

    // Convert latin1 chars to bytes
    const bytes = new Uint8Array(streamData.length)
    for (let i = 0; i < streamData.length; i++) {
      bytes[i] = streamData.charCodeAt(i) & 0xff
    }

    const decompressed = await tryDecompress(bytes)
    if (!decompressed) continue

    const cmapText = toUtf8(decompressed)
    // Parse beginbfchar...endbfchar blocks
    // <CID> <Unicode>
    const bfRegex = /<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>/g
    let bfMatch
    while ((bfMatch = bfRegex.exec(cmapText)) !== null) {
      const cid = bfMatch[1].toUpperCase()
      const unicode = String.fromCharCode(parseInt(bfMatch[2], 16))
      // Key: fontName:cid or just cid
      cidMap.set(fontName + ':' + cid, unicode)
      cidMap.set(cid, unicode)
    }

    // Parse beginbfrange...endbfrange blocks
    // <startCID> <endCID> <unicodeBase>
    const brRegex = /<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>/g
    let brMatch
    while ((brMatch = brRegex.exec(cmapText)) !== null) {
      const start = parseInt(brMatch[1], 16)
      const end = parseInt(brMatch[2], 16)
      const base = parseInt(brMatch[3], 16)
      for (let i = start; i <= end; i++) {
        const cid = i.toString(16).toUpperCase().padStart(4, '0')
        const unicode = String.fromCharCode(base + (i - start))
        cidMap.set(fontName + ':' + cid, unicode)
        cidMap.set(cid, unicode)
      }
    }
  }

  return cidMap
}

function extractStreamBytes(rawBytes: Uint8Array, objMarker: string): Uint8Array | null {
  const text = new TextDecoder('utf-8').decode(rawBytes)
  const objIdx = text.indexOf(objMarker)
  if (objIdx < 0) return null

  // Find byte offset of objMarker in raw bytes
  let bytePos = 0
  let charPos = 0
  const encoder = new TextEncoder()
  while (charPos < objIdx) {
    const ch = text[charPos]
    bytePos += encoder.encode(ch).length
    charPos++
  }

  // Find 'stream' bytes
  const streamMarker = new TextEncoder().encode('stream')
  let streamPos = -1
  for (let i = bytePos; i < rawBytes.length - streamMarker.length; i++) {
    let ok = true
    for (let j = 0; j < streamMarker.length; j++) {
      if (rawBytes[i + j] !== streamMarker[j]) { ok = false; break }
    }
    if (ok) { streamPos = i; break }
  }
  if (streamPos < 0) return null

  let dataStart = streamPos + streamMarker.length
  if (rawBytes[dataStart] === 13) dataStart++
  if (rawBytes[dataStart] === 10) dataStart++

  // Find length
  const headerText = text.substring(objIdx, text.indexOf('stream', objIdx))
  const lenMatch = headerText.match(/\/Length\s+(\d+)/)
  const length = lenMatch ? parseInt(lenMatch[1]) : 10000

  if (dataStart + length > rawBytes.length) return null
  return rawBytes.slice(dataStart, dataStart + length)
}

async function decompressPdfContentStreams(raw: string, rawBytes: Uint8Array): Promise<string[]> {
  const results: string[] = []
  results.push(raw)

  // Find all FlateDecode content streams using regex on latin1 text
  const streamRegex = /(\d+ \d+ obj[\s\S]*?\/Filter\s*\/FlateDecode[\s\S]*?)stream\r?\n([\s\S]*?)endstream/g
  let match
  while ((match = streamRegex.exec(raw)) !== null) {
    const header = match[1]
    let streamData = match[2]

    const lenMatch = header.match(/\/Length\s+(\d+)/)
    if (lenMatch) {
      const length = parseInt(lenMatch[1])
      if (streamData.length > length) {
        streamData = streamData.substring(0, length)
      }
    }

    // Convert latin1 string to bytes for decompression
    const bytes = new Uint8Array(streamData.length)
    for (let i = 0; i < streamData.length; i++) {
      bytes[i] = streamData.charCodeAt(i) & 0xff
    }

    const decompressed = await tryDecompress(bytes)
    if (decompressed) {
      results.push(toUtf8(decompressed))
    }
  }

  return results
}

function decodePdfText(text: string, cidMap: Map<string, string>): string {
  // If no hex patterns, return as-is
  if (!/<[0-9A-Fa-f]+>/.test(text)) return text

  // Replace hex codes with decoded chars
  let decoded = text.replace(/<([0-9A-Fa-f]+)>/g, (_, hex) => {
    const upperHex = hex.toUpperCase()
    return cidMap.get(upperHex) || ''
  })

  // Strip PDF operators and name references
  decoded = decoded
    .replace(/\/[A-Za-z0-9_-]+/g, ' ')  // PDF name references (/FT8, /GS13, etc.)
    .replace(/\b(BT|ET|Tj|TJ|Tf|Td|TD|Tm|T\*|Tc|Tw|Tz|TL|Ts|Tr|gs|rg|RG|cm|q|Q|re|f\*|W\*|n|w|M|J|j|Do|BMC|BDC|EMC|MP|DP|BX|EX|SCN|scn|SC|sc|G|g|CS|cs|i|d|ri|sh)\b/g, ' ')
    .replace(/-?\d+\.?\d*\s+-?\d+\.?\d*\s+(TD|Td|Tm|cm)/g, ' ')
    .replace(/-?\d+\.?\d*/g, ' ')
    .replace(/\s{2,}/g, ' ')
    .trim()

  return decoded
}

async function tryDecompress(bytes: Uint8Array): Promise<Uint8Array | null> {
  for (const format of ['deflate', 'deflate-raw'] as const) {
    try {
      const ds = new DecompressionStream(format)
      const writer = ds.writable.getWriter()
      const reader = ds.readable.getReader()
      writer.write(bytes)
      writer.close()
      const chunks: Uint8Array[] = []
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        if (value) chunks.push(value)
      }
      const totalLen = chunks.reduce((s, c) => s + c.length, 0)
      const result = new Uint8Array(totalLen)
      let pos = 0
      for (const chunk of chunks) {
        result.set(chunk, pos)
        pos += chunk.length
      }
      if (result.length > 0) return result
    } catch {}
  }
  return null
}

function extractPdfText(content: string, out: string[]): void {
  // BT/ET blocks
  const btEtRegex = /BT\s*([\s\S]*?)\s*ET/g
  let match
  while ((match = btEtRegex.exec(content)) !== null) {
    const block = match[1]
    let textBlock = ''

    // Method 1: (text) Tj
    const tjRegex = /\(([^)]*(?:\\.[^)]*)*)\)\s*Tj/g
    let tjMatch
    while ((tjMatch = tjRegex.exec(block)) !== null) {
      textBlock += unescapePdfString(tjMatch[1]) + ' '
    }

    // Method 2: [(text) num (text)] TJ
    const tjArrRegex = /\[([\s\S]*?)\]\s*TJ/g
    let tjArrMatch
    while ((tjArrMatch = tjArrRegex.exec(block)) !== null) {
      const arrContent = tjArrMatch[1]
      const arrTjRegex = /\(([^)]*(?:\\.[^)]*)*)\)/g
      let arrTjMatch
      while ((arrTjMatch = arrTjRegex.exec(arrContent)) !== null) {
        textBlock += unescapePdfString(arrTjMatch[1]) + ' '
      }
    }

    // Method 3: Hex strings <hex> Tj
    const hexRegex = /<([0-9A-Fa-f\s]+)>\s*Tj/g
    let hexMatch
    while ((hexMatch = hexRegex.exec(block)) !== null) {
      const hexStr = hexMatch[1].replace(/\s/g, '')
      try {
        const bytes = new Uint8Array(hexStr.length / 2)
        for (let i = 0; i < hexStr.length; i += 2) {
          bytes[i / 2] = parseInt(hexStr.substring(i, i + 2), 16)
        }
        // Try UTF-16BE (common for CJK) and UTF-8
        const utf16 = tryDecodeBytes(bytes, 'utf-16be')
        if (utf16 && /[\u4e00-\u9fff]/.test(utf16)) {
          textBlock += utf16 + ' '
        } else {
          const utf8 = tryDecodeBytes(bytes, 'utf-8')
          if (utf8) textBlock += utf8 + ' '
        }
      } catch {/* ignore */}
    }

    // Method 4: Hex strings in TJ arrays
    const hexArrRegex = /\[([\s\S]*?)\]\s*TJ/g
    let hexArrMatch
    while ((hexArrMatch = hexArrRegex.exec(block)) !== null) {
      const arrContent = hexArrMatch[1]
      const hexInArrRegex = /<([0-9A-Fa-f\s]+)>/g
      let hMatch
      while ((hMatch = hexInArrRegex.exec(arrContent)) !== null) {
        const hexStr = hMatch[1].replace(/\s/g, '')
        try {
          const bytes = new Uint8Array(hexStr.length / 2)
          for (let i = 0; i < hexStr.length; i += 2) {
            bytes[i / 2] = parseInt(hexStr.substring(i, i + 2), 16)
          }
          const utf16 = tryDecodeBytes(bytes, 'utf-16be')
          if (utf16 && /[\u4e00-\u9fff]/.test(utf16)) {
            textBlock += utf16 + ' '
          } else {
            const utf8 = tryDecodeBytes(bytes, 'utf-8')
            if (utf8) textBlock += utf8 + ' '
          }
        } catch {/* ignore */}
      }
    }

    if (textBlock.trim()) {
      out.push(textBlock.trim())
    }
  }
}

function tryDecodeBytes(bytes: Uint8Array, encoding: string): string | null {
  try {
    const decoded = new TextDecoder(encoding, { fatal: true, ignoreBOM: true }).decode(bytes)
    if (decoded && decoded.trim()) return decoded
  } catch {}
  return null
}

function extractPdfStreams(raw: string): string[] {
  const streams: string[] = []
  // Find obj ... stream ... endstream patterns with FlateDecode
  const objRegex = /(\d+\s+\d+\s+obj[\s\S]*?\/Filter\s*\/FlateDecode[\s\S]*?stream\s*\n?)([\s\S]*?)endstream/g
  let match
  while ((match = objRegex.exec(raw)) !== null) {
    const header = match[1]
    const streamData = match[2]
    
    // Get stream length
    const lenMatch = header.match(/\/Length\s+(\d+)/)
    if (!lenMatch) continue
    
    const length = parseInt(lenMatch[1])
    // Truncate to exact length (strip trailing \n and endstream)
    let data = streamData
    if (data.length > length) {
      data = data.substring(0, length)
    }

    // Decompress if possible (FlateDecode = Deflate)
    try {
      // Convert latin1 string back to bytes
      const bytes = new Uint8Array(data.length)
      for (let i = 0; i < data.length; i++) {
        bytes[i] = data.charCodeAt(i) & 0xff
      }
      // Try to decompress synchronously isn't possible,
      // but we can try using the DecompressionStream pattern
      // For now, store the raw content for text extraction
    } catch {}

    // Even without decompression, try to find text in the raw stream
    // PDF stream content has text operators that might be partially readable
    const textMatch = data.match(/\((?:[^)]|\\[()\\])*\)/g)
    if (textMatch) {
      const text = textMatch.map(t => unescapePdfString(t.slice(1, -1))).join(' ')
      if (text.trim()) streams.push(text)
    }
  }
  return streams
}

function unescapePdfString(s: string): string {
  return s
    .replace(/\\([()\\])/g, '$1')
    .replace(/\\([nrt])/g, (_, c) => {
      if (c === 'n') return '\n'
      if (c === 'r') return '\r'
      if (c === 't') return '\t'
      return ''
    })
    .trim()
}

// --- PPTX ---
async function parsePptx(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)
  const slideFiles = Object.keys(zip)
    .filter(k => k.startsWith('ppt/slides/slide') && k.endsWith('.xml'))
    .sort()

  if (!slideFiles.length) return fallbackDecode(data, 'pptx')

  const texts: string[] = []
  for (let i = 0; i < slideFiles.length; i++) {
    const file = slideFiles[i]
    const xml = toUtf8(zip[file])
    const slideText = extractXmlText(xml, 'a:t').join('\n')
    if (slideText.trim()) {
      texts.push(`[Slide ${i + 1}]\n${slideText}`)
    }
  }
  return texts.join('\n\n')
}

// --- ODT ---
async function parseOdt(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)
  const contentXml = zip['content.xml']
  if (!contentXml) return fallbackDecode(data, 'odt')

  const xml = toUtf8(contentXml)
  const text = xml
    .replace(/<[^>]+>/g, '\n')
    .replace(/&apos;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
  return text
}

// --- EPUB ---
async function parseEpub(data: ArrayBuffer): Promise<string> {
  const zip = await readZip(data)
  const htmlFiles = Object.keys(zip).filter(
    k => (k.endsWith('.html') || k.endsWith('.xhtml') || k.endsWith('.htm')) &&
         !k.includes('nav') && !k.includes('toc')
  )

  if (htmlFiles.length === 0) return fallbackDecode(data, 'epub')

  const texts: string[] = []
  for (const file of htmlFiles) {
    const html = toUtf8(zip[file])
    const text = extractXmlText(html, 'p,h1,h2,h3,h4,h5,h6,li,td,th,div,span,a,blockquote,pre', 'body,section,article').join('\n')
    if (text.trim()) texts.push(text)
  }
  return texts.join('\n\n')
}

// --- Helpers ---

function extractXmlText(xml: string, tagNames: string, containerTag?: string): string[] {
  // If container specified, extract only within containers
  let searchXml = xml
  if (containerTag) {
    const containerNames = containerTag.split(',')
    const parts: string[] = []
    for (const cn of containerNames) {
      const regex = new RegExp(`<${cn}[^>]*>([\\s\\S]*?)<\\/${cn}>`, 'gi')
      let m
      while ((m = regex.exec(xml)) !== null) {
        parts.push(m[1])
      }
    }
    if (parts.length) {
      return parts.flatMap(p => extractXmlText(p, tagNames))
    }
  }

  const results: string[] = []
  const tags = tagNames.split(',')
  for (const tag of tags) {
    // Self-closing or regular
    const regex = new RegExp(`<${tag}(?:\\s[^>]*)?>([\\s\\S]*?)<\\/${tag}>|<${tag}(?:\\s[^>]*)?\\/>`, 'gi')
    let match
    while ((match = regex.exec(searchXml)) !== null) {
      const content = match[1] || ''
      // Nested tags: strip inner XML tags
      const text = content.replace(/<[^>]+>/g, '').trim()
      if (text) results.push(text)
    }
  }

  return results
}

function fallbackDecode(data: ArrayBuffer, format: string): string {
  return decodeUtf8(data)
}

// --- ZIP Reader ---
async function readZip(data: ArrayBuffer): Promise<Record<string, Uint8Array>> {
  const view = new DataView(data)
  const files: Record<string, Uint8Array> = {}

  // Find End of Central Directory
  let eocdOffset = data.byteLength - 22
  while (eocdOffset >= 0) {
    if (view.getUint32(eocdOffset, true) === 0x06054b50) break
    eocdOffset--
  }
  if (eocdOffset < 0) return files

  const centralDirOffset = view.getUint32(eocdOffset + 16, true)
  let offset = centralDirOffset

  while (offset < eocdOffset) {
    const sig = view.getUint32(offset, true)
    if (sig !== 0x02014b50) break

    const compMethod = view.getUint16(offset + 10, true)
    const compSize = view.getUint32(offset + 20, true)
    const uncompSize = view.getUint32(offset + 24, true)
    const nameLen = view.getUint16(offset + 28, true)
    const extraLen = view.getUint16(offset + 30, true)
    const commentLen = view.getUint16(offset + 32, true)
    const localOffset = view.getUint32(offset + 42, true)

    const nameBytes = new Uint8Array(data.slice(offset + 46, offset + 46 + nameLen))
    const fileName = new TextDecoder('utf-8').decode(nameBytes)

    // Read local file header to get actual data location
    const localNameLen = view.getUint16(localOffset + 26, true)
    const localExtraLen = view.getUint16(localOffset + 28, true)
    const dataStart = localOffset + 30 + localNameLen + localExtraLen

    if (compSize > 0 && dataStart + compSize <= data.byteLength) {
      const rawData = new Uint8Array(data.slice(dataStart, dataStart + compSize))

      if (compMethod === 0) {
        // Stored (no compression)
        files[fileName] = rawData
      } else if (compMethod === 8) {
        // Deflate compressed - decompress
        try {
          files[fileName] = await inflateDeflate(rawData, uncompSize)
        } catch (e) {
          console.log('[readZip] deflate decompress failed for', fileName, String(e))
          files[fileName] = rawData
        }
      } else {
        // Unknown compression - store as-is
        console.log('[readZip] unknown compression method', compMethod, 'for', fileName)
        files[fileName] = rawData
      }
    }

    offset += 46 + nameLen + extraLen + commentLen
  }

  return files
}

async function inflateDeflate(compressed: Uint8Array, uncompSize: number): Promise<Uint8Array> {
  // Try DecompressionStream with deflate-raw (ZIP uses raw deflate, no zlib header)
  try {
    const ds = new DecompressionStream('deflate-raw')
    const writer = ds.writable.getWriter()
    const reader = ds.readable.getReader()

    writer.write(compressed)
    writer.close()

    const chunks: Uint8Array[] = []
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      if (value) chunks.push(value)
    }

    const totalLen = chunks.reduce((s, c) => s + c.length, 0)
    const result = new Uint8Array(totalLen)
    let pos = 0
    for (const chunk of chunks) {
      result.set(chunk, pos)
      pos += chunk.length
    }
    return result
  } catch {
    // Fallback: try deflate (with zlib header) in case the file uses that
    try {
      const ds = new DecompressionStream('deflate')
      const writer = ds.writable.getWriter()
      const reader = ds.readable.getReader()
      writer.write(compressed)
      writer.close()
      const chunks: Uint8Array[] = []
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        if (value) chunks.push(value)
      }
      const totalLen = chunks.reduce((s, c) => s + c.length, 0)
      const result = new Uint8Array(totalLen)
      let pos = 0
      for (const chunk of chunks) {
        result.set(chunk, pos)
        pos += chunk.length
      }
      return result
    } catch {
      throw new Error('deflate decompression not supported')
    }
  }
}
