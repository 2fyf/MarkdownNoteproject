import { invoke, isTauri } from '@tauri-apps/api/core'

export const desktopAvailable = isTauri()

export function describeError(error) {
  const text = error?.message || String(error)
  if (/command.*not found|unknown command|not allowed.*course_|course_.*not found/i.test(text)) {
    return '当前程序版本不支持此功能，请更新本地程序后重试。'
  }
  const reasons = {
    COURSE_INVALID_INPUT: '输入不符合要求，请检查关键词或标签长度。',
    COURSE_NOTE_NOT_FOUND: '笔记已不存在，请刷新列表。',
    COURSE_NOTE_NOT_EDITABLE: '此文件不能作为 Markdown 笔记编辑。',
    COURSE_STORAGE_ERROR: '本地数据读写失败，请检查磁盘空间和文件权限后重试。',
  }
  for (const [code, reason] of Object.entries(reasons)) {
    if (text.includes(code)) return `${reason}（${text}）`
  }
  return text
}

export async function call(command, args) {
  if (!desktopAvailable) throw new Error('当前为浏览器界面预览。请使用桌面版读写本地笔记。')
  try {
    return await invoke(command, args)
  } catch (error) {
    throw new Error(describeError(error))
  }
}

// Keep the four commands and their payloads identical to 开发提示词.md.
export const notesApi = {
  workspace: () => call('load_last_workspace'),
  collections: () => call('load_collections'),
  create: (title, collectionId) => call('create_note_source', { input: { title, bodyMd: '', collectionId } }),
  read: (documentId) => call('load_note_source', { documentId }),
  update: (input) => call('update_note_source', { input }),
  remove: (documentId) => call('delete_document', { documentId }),
  move: (documentId, collectionId) => call('move_document_to_collection', { input: { documentId, collectionId } }),
  createCollection: (name) => call('create_collection', { input: { name, parentId: null } }),
  renameCollection: (id, name) => call('rename_collection', { input: { id, name } }),
  deleteCollection: (id) => call('delete_collection', { id }),
  chooseFiles: () => call('choose_pdf_files'),
  importFiles: (sourcePaths, targetCollectionId) => call('import_workspace_paths', { args: { sourcePaths, targetCollectionId } }),
  exportFile: (defaultName, content) => call('export_markdown_file', { defaultName, content }),
  search: (query, tag) => call('course_search_notes', { input: { query, tag: tag || null, limit: 50 } }),
  loadTags: (documentId) => call('course_load_note_tags', { documentId }),
  saveTags: (documentId, tags) => call('course_save_note_tags', { input: { documentId, tags } }),
  listTags: () => call('course_list_tags'),
}
