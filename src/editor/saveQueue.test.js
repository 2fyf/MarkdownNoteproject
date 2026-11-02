import test from 'node:test'
import assert from 'node:assert/strict'
import { createSaveQueue } from './saveQueue.js'

const deferred = () => {
  let resolve
  const promise = new Promise((done) => { resolve = done })
  return { promise, resolve }
}
test('edits during an in-flight write are persisted serially; every caller waits for the newest text', async () => {
  let current = 'first'
  let saved = ''
  const writes = []
  const first = deferred()
  const second = deferred()
  const queue = createSaveQueue({
    read: () => ({ bodyMd: current }),
    isDirty: () => current !== saved,
    write: async (snapshot) => { writes.push(snapshot.bodyMd); await (writes.length === 1 ? first.promise : second.promise) },
    acknowledge: (snapshot) => { saved = snapshot.bodyMd },
  })
  const a = queue.flush()
  current = 'second'
  const b = queue.flush()
  assert.equal(a, b)
  assert.deepEqual(writes, ['first'])
  first.resolve()
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(saved, 'first')
  assert.deepEqual(writes, ['first', 'second'])
  second.resolve()
  await b
  assert.equal(saved, 'second')
})
test('storage failures reject navigation/export flush and retain dirty text for a later retry', async () => {
  let saved = ''
  let fail = true
  const queue = createSaveQueue({
    read: () => ({ bodyMd: '不能丢失的内容' }),
    isDirty: () => saved !== '不能丢失的内容',
    write: async () => { if (fail) throw new Error('磁盘已满') },
    acknowledge: (snapshot) => { saved = snapshot.bodyMd },
  })
  await assert.rejects(queue.flush(), /磁盘已满/)
  assert.equal(saved, '')
  fail = false
  await queue.flush()
  assert.equal(saved, '不能丢失的内容')
})
test('an unchanged note does not start a storage write', async () => {
  const queue = createSaveQueue({ read: () => ({}), isDirty: () => false, write: () => assert.fail('unexpected write'), acknowledge: () => assert.fail('unexpected acknowledgement') })
  await queue.flush()
  await queue.flush()
})
