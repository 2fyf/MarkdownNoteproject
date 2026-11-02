import test from 'node:test'
import assert from 'node:assert/strict'
import { EditorState } from '@codemirror/state'
import { markdownTransaction } from './markdownActions.js'

const apply = (doc, from, to, action) => {
  const state = EditorState.create({ doc, selection: { anchor: from, head: to } })
  return state.update(markdownTransaction(state, action)).state
}
test('inline formatting preserves selected Chinese and supports toggling without duplicate markers', () => {
  const result = apply('前中文后', 1, 3, 'bold')
  assert.equal(result.doc.toString(), '前**中文**后')
  assert.equal(result.sliceDoc(result.selection.main.from, result.selection.main.to), '中文')
  const restored = result.update(markdownTransaction(result, 'bold')).state
  assert.equal(restored.doc.toString(), '前中文后')
})
test('a line selection ending at the next line start leaves the next line untouched', () => {
  const result = apply('甲\n乙\n丙', 0, 4, 'ordered')
  assert.equal(result.doc.toString(), '1. 甲\n2. 乙\n丙')
})
test('list toggling removes existing list prefixes without deleting content', () => {
  assert.equal(apply('- 甲\n- 乙', 0, 7, 'unordered').doc.toString(), '甲\n乙')
})
test('code fences remain valid when selected code contains three backticks', () => {
  const result = apply('前```后', 1, 4, 'code')
  assert.equal(result.doc.toString(), '前\n````\n```\n````\n后')
})
test('table insertion preserves the selected paragraph and uses a valid two-column separator', () => {
  const result = apply('不要覆盖这段话', 0, 7, 'table')
  assert.match(result.doc.toString(), /^不要覆盖这段话\n\n\| 列一 \| 列二 \|\n\| --- \| --- \|/)
})
