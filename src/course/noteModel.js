export const NOTE_TYPES = new Set(['note', 'markdown', 'text'])
export const unicodeLength = (value) => Array.from(value).length

export function normalizeDocuments(snapshot) {
  const unique = new Map()
  for (const root of snapshot?.roots || []) {
    for (const raw of root.documents || []) {
      const contentType = raw.contentType ?? raw.content_type
      if (!NOTE_TYPES.has(contentType)) continue
      unique.set(raw.id, {
        id: raw.id,
        title: raw.title || '无标题笔记',
        contentType,
        collectionId: raw.collectionId ?? raw.collection_id ?? null,
        modified: raw.modified || '',
        indexStatus: raw.indexStatus ?? raw.index_status ?? '',
      })
    }
  }
  return [...unique.values()].sort((a, b) => String(b.modified).localeCompare(String(a.modified)) || a.id.localeCompare(b.id))
}

export function collectionPath(collections, id) {
  const parts = []
  const seen = new Set()
  let current = collections.find((item) => item.id === id)
  while (current && !seen.has(current.id)) {
    seen.add(current.id)
    parts.unshift(current.name)
    current = collections.find((item) => item.id === current.parentId)
  }
  return parts.join(' / ')
}

export function normalizeTags(tags) {
  if (!Array.isArray(tags)) throw new Error('标签列表格式不正确。')
  const result = []
  const seen = new Set()
  for (const raw of tags) {
    const tag = String(raw).trim()
    if (!tag || unicodeLength(tag) > 20 || /[\u0000-\u001f\u007f-\u009f]/u.test(tag)) {
      throw new Error('标签须为 1～20 个字符，不能包含换行或控制字符。')
    }
    const key = tag.replace(/[A-Z]/g, (char) => char.toLowerCase())
    if (!seen.has(key)) { seen.add(key); result.push(tag) }
  }
  if (result.length > 10) throw new Error('每篇笔记最多添加 10 个标签。')
  return result
}

export function splitImportPaths(paths) {
  const accepted = []
  const rejected = []
  for (const path of paths) (/\.(md|markdown|txt|text)$/i.test(path) ? accepted : rejected).push(path)
  return { accepted, rejected }
}

export function exportName(title) {
  const safe = String(title || '无标题笔记').replace(/[<>:"/\\|?*\u0000-\u001f]/g, '_').trim().replace(/[. ]+$/g, '')
  const stem = Array.from(safe || '无标题笔记').slice(0, 80).join('')
  return `${/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(stem) ? '_' : ''}${stem}.md`
}

export function markdownExport({ title, bodyMd }) {
  if (!title?.trim() || /^\s*#\s+[^\r\n]+/u.test(bodyMd || '')) return bodyMd || ''
  const heading = title.trim().replace(/[\r\n]/g, ' ').replace(/([\\`*_{}[\]<>#!|])/g, '\\$1')
  return `# ${heading}\n\n${bodyMd || ''}`
}
