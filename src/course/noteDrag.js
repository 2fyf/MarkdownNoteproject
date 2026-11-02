export const NOTE_DRAG_TYPE = 'application/x-markdownnote-note'
export const TAG_DRAG_TYPE = 'application/x-markdownnote-tag'

export function dragKind(transfer) {
  const types = Array.from(transfer?.types || [])
  if (types.includes(NOTE_DRAG_TYPE)) return 'note'
  if (types.includes(TAG_DRAG_TYPE)) return 'tag'
  return ''
}

export function writeNoteDrag(transfer, note) {
  if (!transfer || !note?.id) return false
  transfer.effectAllowed = 'copyMove'
  transfer.setData(NOTE_DRAG_TYPE, JSON.stringify({ version: 1, id: note.id }))
  return true
}

export function writeTagDrag(transfer, name) {
  if (!transfer || !name?.trim()) return false
  transfer.effectAllowed = 'copy'
  transfer.setData(TAG_DRAG_TYPE, JSON.stringify({ version: 1, name }))
  return true
}

export function readNoteDrag(transfer) {
  try {
    const data = JSON.parse(transfer?.getData(NOTE_DRAG_TYPE) || '')
    return data.version === 1 && typeof data.id === 'string' && data.id.trim() ? { id: data.id } : null
  } catch { return null }
}

export function readTagDrag(transfer) {
  try {
    const data = JSON.parse(transfer?.getData(TAG_DRAG_TYPE) || '')
    return data.version === 1 && typeof data.name === 'string' && data.name.trim() ? { name: data.name } : null
  } catch { return null }
}

export function wikiLinkForNote(note, currentId) {
  if (!note?.id || !note.title?.trim()) throw new Error('拖拽的笔记已不存在，请刷新列表。')
  if (note.id === currentId) throw new Error('请拖入另一篇笔记，不能将当前笔记链接到自身。')
  const title = note.title.trim()
  if (/[\[\]|\r\n]/.test(title)) throw new Error('该笔记标题含有双链分隔符，请先修改标题后再插入链接。')
  return `[[${title}]]`
}
