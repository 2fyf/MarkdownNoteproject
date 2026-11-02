// Literal matching shares the backend's ASCII-only case folding. Offsets stay
// in UTF-16 units, as required by CodeMirror and JavaScript text slicing.
export function normalizeSearchQuery(value) {
  const query = String(value || '').trim()
  return Array.from(query).length <= 100 ? query : ''
}

const asciiLower = value => value.replace(/[A-Z]/g, char => char.toLowerCase())

export function searchMatchRanges(value, rawQuery) {
  const text = String(value || '')
  const query = normalizeSearchQuery(rawQuery)
  if (!query) return []
  const haystack = asciiLower(text)
  const needle = asciiLower(query)
  const ranges = []
  let start = 0
  while (start <= haystack.length - needle.length) {
    const from = haystack.indexOf(needle, start)
    if (from < 0) break
    ranges.push({ from, to: from + needle.length })
    start = from + needle.length
  }
  return ranges
}

export function searchTextParts(value, query) {
  const text = String(value || '')
  const parts = []
  let offset = 0
  for (const { from, to } of searchMatchRanges(text, query)) {
    if (from > offset) parts.push({ text: text.slice(offset, from), matched: false })
    parts.push({ text: text.slice(from, to), matched: true })
    offset = to
  }
  if (offset < text.length || !parts.length) parts.push({ text: text.slice(offset), matched: false })
  return parts
}

// The caller supplies already-sanitized HTML. Only text nodes are decorated,
// never attributes/URLs, and query strings are never interpreted as markup.
export function highlightHtmlText(sanitizedHtml, query, ownerDocument = globalThis.document) {
  if (!normalizeSearchQuery(query) || !ownerDocument) return sanitizedHtml
  const host = ownerDocument.createElement('div')
  host.innerHTML = sanitizedHtml
  const walker = ownerDocument.createTreeWalker(host, 4 /* SHOW_TEXT */)
  const nodes = []
  while (walker.nextNode()) nodes.push(walker.currentNode)
  for (const node of nodes) {
    if (node.parentElement?.namespaceURI !== 'http://www.w3.org/1999/xhtml'
      || node.parentElement?.closest('script, style, textarea, .katex-mathml')) continue
    const parts = searchTextParts(node.nodeValue, query)
    if (!parts.some(part => part.matched)) continue
    const fragment = ownerDocument.createDocumentFragment()
    for (const part of parts) {
      if (part.matched) {
        const mark = ownerDocument.createElement('mark')
        mark.className = 'course-search-match'
        mark.textContent = part.text
        fragment.append(mark)
      } else fragment.append(ownerDocument.createTextNode(part.text))
    }
    node.replaceWith(fragment)
  }
  return host.innerHTML
}
