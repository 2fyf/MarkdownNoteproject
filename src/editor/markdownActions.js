// A single CodeMirror transaction keeps toolbar edits undoable and preserves the selection.
export const markdownActions = [
  { id: 'heading', label: '二级标题', text: 'H2', key: 'Mod-Alt-2', hint: 'Ctrl+Alt+2' },
  { id: 'bold', label: '粗体', text: 'B', key: 'Mod-b', hint: 'Ctrl+B' },
  { id: 'italic', label: '斜体', text: 'I', key: 'Mod-i', hint: 'Ctrl+I' },
  { id: 'unordered', label: '无序列表', text: '• 列表', key: 'Mod-Shift-8', hint: 'Ctrl+Shift+8' },
  { id: 'ordered', label: '有序列表', text: '1. 列表', key: 'Mod-Shift-7', hint: 'Ctrl+Shift+7' },
  { id: 'quote', label: '引用', text: '❞', key: 'Mod-Shift-9', hint: 'Ctrl+Shift+9' },
  { id: 'code', label: '代码块', text: '</>', key: 'Mod-Shift-c', hint: 'Ctrl+Shift+C' },
  { id: 'table', label: '插入表格', text: '表格', key: 'Mod-Alt-t', hint: 'Ctrl+Alt+T' },
  { id: 'link', label: '内部链接', text: '[[链接]]', key: 'Mod-Shift-k', hint: 'Ctrl+Shift+K' },
]

export function markdownTransaction(state, action) {
  const { from, to } = state.selection.main
  const selected = state.doc.sliceString(from, to)
  const replace = (start, end, insert, anchor, head) => ({ changes: { from: start, to: end, insert }, selection: { anchor, head }, userEvent: 'input.format', scrollIntoView: true })
  const inline = { bold: '**', italic: '*', link: '[[' }
  if (inline[action]) {
    const open = inline[action], close = action === 'link' ? ']]' : open
    if (from >= open.length && state.doc.sliceString(from - open.length, from) === open && state.doc.sliceString(to, to + close.length) === close) {
      return replace(from - open.length, to + close.length, selected, from - open.length, to - open.length)
    }
    const content = selected || (action === 'link' ? '笔记标题' : '文字')
    return replace(from, to, open + content + close, from + open.length, from + open.length + content.length)
  }
  const first = state.doc.lineAt(from)
  // Ending at the next line's start should not format that unselected line.
  const last = state.doc.lineAt(to > from && state.doc.lineAt(to).from === to ? to - 1 : to)
  const lines = state.doc.sliceString(first.from, last.to).split('\n')
  const patterns = { heading: /^#{1,6}\s+/, unordered: /^[-*+]\s+/, ordered: /^\d+\.\s+/, quote: /^>\s?/ }
  if (patterns[action]) {
    const pattern = patterns[action]
    const remove = lines.every(line => (action === 'heading' ? /^##\s+/ : pattern).test(line))
    const changed = lines.map((line, index) => {
      const content = line.replace(pattern, '')
      if (remove) return content
      const prefix = action === 'heading' ? '## ' : action === 'unordered' ? '- ' : action === 'ordered' ? `${index + 1}. ` : '> '
      return prefix + content
    }).join('\n')
    return replace(first.from, last.to, changed, first.from, first.from + changed.length)
  }
  if (action === 'code') {
    const content = selected || '在这里输入代码'
    // Use a longer fence when the selection already contains backticks.
    const longest = Math.max(2, ...[...content.matchAll(/`+/g)].map(match => match[0].length))
    const fence = '`'.repeat(longest + 1)
    const prefix = (from > 0 && state.doc.sliceString(from - 1, from) !== '\n' ? '\n' : '') + fence + '\n'
    const suffix = '\n' + fence + (to < state.doc.length && state.doc.sliceString(to, to + 1) !== '\n' ? '\n' : '')
    return replace(from, to, prefix + content + suffix, from + prefix.length, from + prefix.length + content.length)
  }
  if (action === 'table') {
    // Keep arbitrary selected text intact; a template belongs after its last line.
    const prefix = last.to > 0 ? '\n\n' : ''
    const template = '| 列一 | 列二 |\n| --- | --- |\n| 内容 | 内容 |\n'
    const start = last.to + prefix.length + 2
    return replace(last.to, last.to, prefix + template, start, start + 2)
  }
  return null
}
