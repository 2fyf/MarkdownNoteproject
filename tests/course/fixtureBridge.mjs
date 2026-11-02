// TEST ONLY. Never imported by src/. No real files or database are written.
export function installTestBridge() {
  window.isTauri = true
  const state = {
    notes: [
      { id: 'test-a', title: '测试样例 · 软件工程', bodyMd: '# 软件工程\n\n需求分析与设计。\n\n[[测试样例 · 复习计划]]', collectionId: 'test-c', contentType: 'note', tags: ['课程'] },
      { id: 'test-b', title: '测试样例 · 复习计划', bodyMd: '本周复习中文检索与知识管理，进度 100%。', collectionId: null, contentType: 'markdown', tags: ['复习'] },
    ],
    collections: [{ id: 'test-c', parentId: null, name: '测试课程', position: 0 }],
    calls: [], callbacks: {}, listeners: {}, serial: 0,
    failSaves: false, saveDelay: 0, missingTags: false, failTagSaves: false, tagReadDelay: 0, exported: null, destroyed: false, fullscreen: false, failFullscreen: false, failOpenUrl: false, openedUrls: [], failMoves: false,
  }
  const testBody = sessionStorage.getItem('course.test.body')
  if (testBody !== null) state.notes[0].bodyMd = testBody
  if (sessionStorage.getItem('course.test.corruptPosition')) {
    localStorage.setItem('course.editorPositions', 'invalid-json')
    sessionStorage.removeItem('course.test.corruptPosition')
  }
  window.__courseTest = state
  const snapshot = () => ({ root: { id: 'test-root', path: 'test-only://fixture' }, documents: state.notes.map((item) => ({ id: item.id, title: item.title, content_type: item.contentType, collection_id: item.collectionId, modified: '2026-10-27', index_status: 'ready' })) })
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' } },
    transformCallback(callback) { const id = ++state.serial; state.callbacks[id] = callback; return id },
    unregisterCallback(id) { delete state.callbacks[id] },
    async invoke(command, args = {}) {
      state.calls.push({ command, args: structuredClone(args) })
      if (command === 'plugin:event|listen') { state.listeners[args.event] = args.handler; return ++state.serial }
      if (command === 'plugin:event|unlisten') return
      if (command === 'plugin:window|destroy') { state.destroyed = true; return }
      if (command === 'plugin:window|is_fullscreen') return state.fullscreen
      if (command === 'open_external_url') {
        if (state.failOpenUrl) throw new Error('测试注入：默认浏览器不可用')
        state.openedUrls.push(args.url)
        return
      }
      if (command === 'plugin:window|set_fullscreen') {
        if (state.failFullscreen) throw new Error('Fullscreen test failure')
        state.fullscreen = args.value
        return
      }
      if (command === 'load_last_workspace') return { roots: [snapshot()] }
      if (command === 'load_collections') return structuredClone(state.collections)
      if (command === 'course_list_tags') {
        if (state.missingTags) throw new Error('Command course_list_tags not found')
        const tags = new Map()
        for (const note of state.notes) for (const tag of note.tags) tags.set(tag, (tags.get(tag) || 0) + 1)
        return [...tags].map(([name, noteCount]) => ({ name, noteCount }))
      }
      if (command === 'create_note_source') {
        const id = `test-new-${++state.serial}`
        state.notes.push({ id, ...args.input, contentType: 'note', tags: [] })
        return { documentId: id, snapshot: snapshot() }
      }
      if (command === 'load_note_source') {
        const note = state.notes.find((item) => item.id === args.documentId)
        if (!note) throw new Error('COURSE_NOTE_NOT_FOUND')
        return structuredClone(note)
      }
      if (command === 'update_note_source') {
        if (state.saveDelay) await new Promise((resolve) => setTimeout(resolve, state.saveDelay))
        if (state.failSaves) throw new Error('测试注入：磁盘已满')
        Object.assign(state.notes.find((item) => item.id === args.input.documentId), args.input)
        return { documentId: args.input.documentId, status: 'ready' }
      }
      if (command === 'load_note_links') {
        const note = state.notes.find((item) => item.id === args.documentId)
        return {
          outbound: [...note.bodyMd.matchAll(/\[\[([^\]]+)\]\]/g)].map((match) => ({ title: match[1], documentId: state.notes.find((item) => item.title === match[1])?.id || null })),
          backlinks: state.notes.filter((item) => item.id !== note.id && item.bodyMd.includes(`[[${note.title}]]`)).map((item) => ({ title: item.title, documentId: item.id })),
        }
      }
      if (command === 'course_search_notes') {
        const { query, tag, limit } = args.input
        if (query === '软件') await new Promise((resolve) => setTimeout(resolve, 1200))
        const matches = state.notes.filter((item) => (!tag || item.tags.includes(tag)) && `${item.title}\n${item.bodyMd}`.toLowerCase().includes(query.toLowerCase()))
        return { items: matches.slice(0, limit).map((item) => ({ documentId: item.id, title: item.title, snippet: item.bodyMd.slice(0, 120), contentType: item.contentType, tags: item.tags })), total: matches.length, truncated: matches.length > limit }
      }
      if (command === 'course_load_note_tags') {
        if (state.tagReadDelay) await new Promise(resolve => setTimeout(resolve, state.tagReadDelay))
        if (state.missingTags) throw new Error('Command course_load_note_tags not found')
        return [...state.notes.find((item) => item.id === args.documentId).tags]
      }
      if (command === 'course_save_note_tags') {
        if (state.failTagSaves) throw new Error('COURSE_STORAGE_ERROR: 测试注入标签写入失败')
        const note = state.notes.find((item) => item.id === args.input.documentId)
        note.tags = [...args.input.tags]
        return [...note.tags]
      }
      if (command === 'move_document_to_collection') {
        if (state.failMoves) throw new Error('测试注入：分类更新失败')
        state.notes.find((item) => item.id === args.input.documentId).collectionId = args.input.collectionId
        return
      }
      if (command === 'create_collection') {
        const node = { id: `test-collection-${++state.serial}`, ...args.input, position: 0 }
        state.collections.push(node)
        return { ...node }
      }
      if (command === 'rename_collection') { state.collections.find((item) => item.id === args.input.id).name = args.input.name; return }
      if (command === 'delete_collection') {
        state.collections = state.collections.filter((item) => item.id !== args.id)
        for (const note of state.notes) if (note.collectionId === args.id) note.collectionId = null
        return
      }
      if (command === 'delete_document') { state.notes = state.notes.filter((item) => item.id !== args.documentId); return }
      if (command === 'choose_pdf_files') return ['test-only.md', 'unsupported.pdf']
      if (command === 'import_workspace_paths') {
        state.notes.push({ id: 'test-import', title: '测试导入样例', bodyMd: '只存在于前端契约测试内存中。', contentType: 'markdown', tags: [], collectionId: args.args.targetCollectionId })
        return [snapshot()]
      }
      if (command === 'export_markdown_file') { state.exported = { ...args }; return 'test-only://export.md' }
      throw new Error(`Unexpected test command: ${command}`)
    },
  }
  if (!localStorage.getItem('markdownnote.theme')) localStorage.setItem('markdownnote.theme', 'light')
}
