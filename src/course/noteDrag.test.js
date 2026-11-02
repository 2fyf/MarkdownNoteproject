import test from 'node:test'
import assert from 'node:assert/strict'
import { dragKind, readNoteDrag, readTagDrag, writeNoteDrag, writeTagDrag, wikiLinkForNote } from './noteDrag.js'
const transfer = () => ({ types: [], values: new Map(), setData(type, value) { this.values.set(type, value); this.types.push(type) }, getData(type) { return this.values.get(type) || '' } })

test('Internal drags identify only notes and tags, ignoring plain text and files', () => {
  const note = transfer(); writeNoteDrag(note, { id: 'a' })
  assert.equal(dragKind(note), 'note'); assert.deepEqual(readNoteDrag(note), { id: 'a' })
  const tag = transfer(); writeTagDrag(tag, '课程')
  assert.equal(dragKind(tag), 'tag'); assert.deepEqual(readTagDrag(tag), { name: '课程' })
  assert.equal(dragKind({ types: ['text/plain', 'Files'] }), '')
})
test('Malformed drag payloads do not become database IDs', () => {
  assert.equal(readNoteDrag({ getData: () => '{broken' }), null)
  assert.equal(readNoteDrag({ getData: () => JSON.stringify({ version: 2, id: 'a' }) }), null)
  assert.equal(readTagDrag({ getData: () => JSON.stringify({ version: 1, name: [] }) }), null)
})
test('Dropped note links preserve Chinese titles and reject self or ambiguous syntax', () => {
  assert.equal(wikiLinkForNote({ id: 'b', title: ' 复习计划 ' }, 'a'), '[[复习计划]]')
  for (const note of [{ id: 'a', title: '自己' }, { id: 'b', title: '坏|标题' }, { id: 'b', title: '坏]标题' }, {}]) {
    assert.throws(() => wikiLinkForNote(note, 'a'))
  }
})
