import test from 'node:test'
import assert from 'node:assert/strict'
import { normalizeDocuments, collectionPath, normalizeTags, splitImportPaths, exportName, markdownExport } from './noteModel.js'

test('workspace normalization accepts both Rust naming styles and excludes PDF/web', () => {
  const result = normalizeDocuments({ roots: [{ documents: [
    { id: 'a', title: '课程', content_type: 'note', collection_id: 'class', modified: '2026-10-27' },
    { id: 'b', contentType: 'markdown', modified: '2026-10-26' },
    { id: 'pdf', content_type: 'pdf' }, { id: 'web', contentType: 'web' },
  ] }, { documents: [{ id: 'a', title: '新标题', contentType: 'note', collectionId: 'class', modified: '2026-10-27' }] }] })
  assert.equal(result.length, 2)
  assert.equal(result[0].title, '新标题')
  assert.equal(result[0].collectionId, 'class')
  assert.deepEqual(normalizeDocuments(null), [])
})
test('nested collection labels tolerate a corrupt parent cycle', () => {
  const nodes = [{ id: 'a', parentId: null, name: '课程' }, { id: 'b', parentId: 'a', name: '实验' }]
  assert.equal(collectionPath(nodes, 'b'), '课程 / 实验')
  nodes[0].parentId = 'b'
  assert.equal(collectionPath(nodes, 'b'), '课程 / 实验')
})
test('tags enforce Unicode length, ASCII dedup, controls and maximum count', () => {
  assert.deepEqual(normalizeTags([' 课程 ', 'API', 'api', '课程']), ['课程', 'API'])
  assert.deepEqual(normalizeTags(['😀'.repeat(20)]), ['😀'.repeat(20)])
  assert.throws(() => normalizeTags(['😀'.repeat(21)]), /1～20/)
  assert.throws(() => normalizeTags(['中\n文']), /控制字符/)
  assert.throws(() => normalizeTags(['   ']), /1～20/)
  assert.throws(() => normalizeTags(Array.from({ length: 11 }, (_, i) => String(i))), /10/)
  assert.deepEqual(normalizeTags([]), [])
})
test('import picker rejects documents outside the note scope before IPC', () => {
  assert.deepEqual(splitImportPaths(['D:\\课程.MD', 'D:\\file.txt', 'bad.pdf', 'bad.docx']), { accepted: ['D:\\课程.MD', 'D:\\file.txt'], rejected: ['bad.pdf', 'bad.docx'] })
})
test('export names work on Windows and export preserves a title without duplicating H1', () => {
  assert.equal(exportName('CON'), '_CON.md')
  assert.equal(exportName('我的:笔记/计划?'), '我的_笔记_计划_.md')
  assert.equal(exportName(' .. '), '无标题笔记.md')
  assert.equal(markdownExport({ title: '课程', bodyMd: '正文' }), '# 课程\n\n正文')
  assert.equal(markdownExport({ title: '课程', bodyMd: '# 已有标题\n\n正文' }), '# 已有标题\n\n正文')
})
