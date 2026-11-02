// A successful write only acknowledges its own snapshot, never later edits.
export function createSaveQueue({ read, write, acknowledge, isDirty }) {
  let active = null
  function flush() {
    if (active) return active
    active = (async () => {
      while (isDirty()) {
        const snapshot = { ...read() }
        await write(snapshot)
        acknowledge(snapshot)
      }
    })().finally(() => { active = null })
    return active
  }
  return { flush }
}
