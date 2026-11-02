import test from 'node:test'
import assert from 'node:assert/strict'
import { normalizeSearchQuery, searchMatchRanges, searchTextParts } from './searchHighlight.js'

test('Chinese body hits and ASCII case folding preserve exact display text', () => {
  assert.deepEqual(searchMatchRanges('中文复习与复习', '复习'), [{ from: 2, to: 4 }, { from: 5, to: 7 }])
  assert.deepEqual(searchTextParts('CodeMirror codemirror', 'CODEMIRROR').filter(part => part.matched).map(part => part.text), ['CodeMirror', 'codemirror'])
  assert.deepEqual(searchMatchRanges('Ä ä', 'ä'), [{ from: 2, to: 3 }])
})

test('Search symbols are literal, never regex or HTML', () => {
  assert.deepEqual(searchMatchRanges('100%_ [a.*] %_', '%_'), [{ from: 3, to: 5 }, { from: 12, to: 14 }])
  assert.deepEqual(searchTextParts('<img onerror=alert(1)>', '<img').filter(part => part.matched).map(part => part.text), ['<img'])
  assert.deepEqual(searchMatchRanges('one two', '.*'), [])
})

test('Unicode offsets work with the editor and clearing a query removes all marks', () => {
  assert.deepEqual(searchMatchRanges('😀复习😀', '复习'), [{ from: 2, to: 4 }])
  assert.deepEqual(searchMatchRanges('abc', ''), [])
  assert.deepEqual(searchTextParts('原文', ' '), [{ text: '原文', matched: false }])
  assert.equal(normalizeSearchQuery('词'.repeat(101)), '')
  assert.equal(normalizeSearchQuery('😀'.repeat(100)), '😀'.repeat(100))
  const text = '复习复习计划'
  assert.equal(searchTextParts(text, '复习').map(part => part.text).join(''), text)
})
