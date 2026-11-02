<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import NoteEditor from '../components/NoteEditor.vue'
import Icon from './components/CourseIcon.vue'
import SearchHighlight from './components/SearchHighlight.vue'
import { notesApi, desktopAvailable, describeError } from './notesApi.js'
import { normalizeDocuments, collectionPath, normalizeTags, splitImportPaths, unicodeLength, exportName, markdownExport } from './noteModel.js'
import { resolvedTheme, setThemePreference } from '../theme.js'
import { useWorkspaceLayout } from './useWorkspaceLayout.js'
import { dragKind, writeNoteDrag, writeTagDrag, readNoteDrag, readTagDrag } from './noteDrag.js'

const documents = ref([])
const collections = ref([])
const allTags = ref([])
const collectionFilter = ref('all')
const tagFilter = ref('')
const query = ref('')
const results = ref([])
const resultTotal = ref(0)
const resultTruncated = ref(false)
const searching = ref(false)
const searchError = ref('')
const tagsError = ref('')
const noteTagsError = ref('')
const noteTags = ref([])
const tagDraft = ref('')
const tagsLoading = ref(false)
const selectedId = ref('')
const dirty = ref(false)
const editor = ref(null)
const searchInput = ref(null)
const busy = ref('')
const initialLoading = ref(false)
const workspaceError = ref('')
const alert = ref(null)
const detailOpen = ref(true)
const dialog = ref(null)
const dialogElement = ref(null)
const dialogField = ref(null)
const contextMenu = ref(null)
const menuElement = ref(null)
const itemDrag = ref(null)
const dropTarget = ref('')
let searchTimer
let alertTimer
let searchVersion = 0
let tagVersion = 0
let disposed = false
let stopIndexListener
let stopCloseListener
let closeAllowed = false
let previousFocus
let menuOrigin
const { sidebarHidden, listHidden, focusMode, fullscreen, changingFullscreen, focused, navigationVisible, listVisible, detailsVisible, dragging, sidebarWidth, listWidth, sidebarMax, listMax, layoutStyle, startResize, resize, endResize, resizeKey, resetWidth, toggleFullscreen } = useWorkspaceLayout({ detailOpen, desktopAvailable, notify })

const selected = computed(() => documents.value.find((item) => item.id === selectedId.value))
const filtered = computed(() => Boolean(query.value.trim() || tagFilter.value))
const collectionName = computed(() => collectionFilter.value === 'all' ? '全部笔记' : collectionFilter.value === 'unfiled' ? '未分类' : collectionPath(collections.value, collectionFilter.value))
const collectionOptions = computed(() => collections.value.map((item) => ({ ...item, label: collectionPath(collections.value, item.id) })))
const listed = computed(() => {
  const source = filtered.value ? results.value.map((item) => {
    const stored = documents.value.find((doc) => doc.id === item.documentId)
    return { ...stored, id: item.documentId, title: item.title, snippet: item.snippet, tags: item.tags || [] }
  }) : documents.value
  if (collectionFilter.value === 'all') return source
  if (collectionFilter.value === 'unfiled') return source.filter((item) => !item.collectionId)
  return source.filter((item) => item.collectionId === collectionFilter.value)
})
const unavailable = computed(() => !desktopAvailable || initialLoading.value || Boolean(workspaceError.value))
const noteCollection = computed(() => selected.value?.collectionId ? collectionPath(collections.value, selected.value.collectionId) : '未分类')
const menuItems = computed(() => contextMenu.value?.kind === 'collection' ? [
  { key: 'create', label: '在此分类新建笔记', icon: 'plus' },
  { key: 'rename', label: '重命名分类', icon: 'edit' },
  { key: 'deleteCollection', label: '删除分类', icon: 'trash', danger: true, separator: true },
] : [
  { key: 'open', label: '打开笔记', icon: 'file' },
  { key: 'renameNote', label: '重命名', icon: 'edit' },
  { key: 'moveNote', label: '移动到分类…', icon: 'folder' },
  { key: 'tags', label: '管理标签…', icon: 'tag' },
  { key: 'export', label: '导出 Markdown…', icon: 'download', separator: true },
  { key: 'deleteNote', label: '删除笔记…', icon: 'trash', danger: true, separator: true },
])

function closeMenu(restoreFocus = false) {
  contextMenu.value = null
  if (restoreFocus && menuOrigin?.isConnected) menuOrigin.focus()
}
async function showMenu(event, kind, item) {
  event.preventDefault()
  if (busy.value || unavailable.value || dialog.value) return
  menuOrigin = event.currentTarget
  const bounds = menuOrigin.getBoundingClientRect()
  const keyboard = event.type === 'keydown' || (!event.clientX && !event.clientY)
  contextMenu.value = { kind, id: item.id, title: item.title || item.name, x: keyboard ? bounds.left + 12 : event.clientX, y: keyboard ? bounds.bottom : event.clientY }
  await nextTick()
  if (!menuElement.value || !contextMenu.value) return
  const menuBounds = menuElement.value.getBoundingClientRect()
  contextMenu.value.x = Math.max(8, Math.min(contextMenu.value.x, window.innerWidth - menuBounds.width - 8))
  contextMenu.value.y = Math.max(8, Math.min(contextMenu.value.y, window.innerHeight - menuBounds.height - 8))
  menuElement.value.querySelector('[role="menuitem"]')?.focus()
}
function menuKeydown(event) {
  const items = [...menuElement.value.querySelectorAll('[role="menuitem"]')]
  const current = items.indexOf(document.activeElement)
  let index
  if (event.key === 'ArrowDown') index = (current + 1) % items.length
  else if (event.key === 'ArrowUp') index = (current - 1 + items.length) % items.length
  else if (event.key === 'Home') index = 0
  else if (event.key === 'End') index = items.length - 1
  else if (event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); event.stopPropagation(); closeMenu(true); return }
  else return
  event.preventDefault()
  items[index]?.focus()
}
function dismissMenu(event) {
  if (contextMenu.value && !menuElement.value?.contains(event.target)) closeMenu()
}
function chooseMenu(key) {
  const target = contextMenu.value
  closeMenu(true)
  if (!target || busy.value) return
  if (key === 'open') selectNote(target.id)
  else if (key === 'export') exportNote(target.id)
  else if (key === 'create') openDialog('create', '新建笔记', { collectionId: target.id })
  else openDialog(key, key.startsWith('rename') ? target.title : '', { targetId: target.id, targetTitle: target.title })
}
watch([collectionFilter, query, tagFilter], () => closeMenu())

function notify(text, type = 'success') {
  clearTimeout(alertTimer)
  alert.value = { text, type }
  if (type === 'success') alertTimer = setTimeout(() => { alert.value = null }, 6000)
}
async function action(label, fn) {
  if (busy.value || disposed) return
  busy.value = label
  try { await fn() }
  catch (error) { notify(describeError(error), 'error') }
  finally { busy.value = '' }
}
async function flushCurrent() {
  if (dirty.value) {
    if (!editor.value) throw new Error('编辑器尚未准备好，请稍后重试。')
    await editor.value.flushSave()
  }
}
async function refreshWorkspace() {
  const [snapshot, nodes] = await Promise.all([notesApi.workspace(), notesApi.collections()])
  if (disposed) return
  documents.value = normalizeDocuments(snapshot)
  collections.value = nodes
  workspaceError.value = ''
  if (selectedId.value && !documents.value.some((item) => item.id === selectedId.value)) {
    selectedId.value = ''
    dirty.value = false
  }
  if (!['all', 'unfiled'].includes(collectionFilter.value) && !nodes.some((item) => item.id === collectionFilter.value)) collectionFilter.value = 'all'
}
async function loadWorkspace() {
  if (!desktopAvailable) return
  initialLoading.value = true
  try { await refreshWorkspace() }
  catch (error) { workspaceError.value = describeError(error) }
  finally { initialLoading.value = false }
  void refreshTags()
}
function reload() {
  void action('正在刷新…', async () => { await flushCurrent(); await loadWorkspace(); scheduleSearch() })
}
async function refreshTags() {
  if (!desktopAvailable) return
  try { const tags = await notesApi.listTags(); if (!disposed) { allTags.value = tags; tagsError.value = '' } }
  catch (error) { if (!disposed) tagsError.value = describeError(error) }
}
async function loadNoteTags() {
  const version = ++tagVersion
  const id = selectedId.value
  noteTags.value = []
  noteTagsError.value = ''
  tagDraft.value = ''
  if (!id) return
  tagsLoading.value = true
  try {
    const tags = await notesApi.loadTags(id)
    if (!disposed && version === tagVersion) noteTags.value = tags
  } catch (error) { if (!disposed && version === tagVersion) noteTagsError.value = describeError(error) }
  finally { if (version === tagVersion) tagsLoading.value = false }
}
watch(selectedId, loadNoteTags)
watch(selectedId, id => {
  try { if (id) localStorage.setItem('course.lastNoteId', id); else localStorage.removeItem('course.lastNoteId') }
  catch { /* Optional view preference, not note storage. */ }
})

function scheduleSearch() {
  clearTimeout(searchTimer)
  ++searchVersion // Invalidate a request immediately, before the debounce.
  results.value = []
  searchError.value = ''
  if (!filtered.value || !desktopAvailable) { searching.value = false; return }
  searching.value = true
  searchTimer = setTimeout(runSearch, 300)
}
async function runSearch() {
  clearTimeout(searchTimer)
  const version = ++searchVersion
  if (!filtered.value || !desktopAvailable) { searching.value = false; return }
  const text = query.value.trim()
  if (unicodeLength(text) > 100) {
    searchError.value = '关键词最多 100 个字符。'
    searching.value = false
    return
  }
  searching.value = true
  searchError.value = ''
  try {
    await flushCurrent()
    if (version !== searchVersion || disposed) return
    const data = await notesApi.search(text, tagFilter.value)
    if (version !== searchVersion || disposed) return
    results.value = data.items
    resultTotal.value = data.total
    resultTruncated.value = data.truncated
  } catch (error) { if (version === searchVersion && !disposed) searchError.value = describeError(error) }
  finally { if (version === searchVersion) searching.value = false }
}
watch([query, tagFilter], scheduleSearch)

function selectNote(id) {
  if (id === selectedId.value) return
  void action('正在切换笔记…', async () => { await flushCurrent(); selectedId.value = id; dirty.value = false })
}
function onSaved(data) {
  const doc = documents.value.find((item) => item.id === data.documentId)
  if (doc) { doc.title = data.title; doc.modified = new Date().toISOString() }
  // A fresh save can change the fulltext result as well as its title.
  if (filtered.value) scheduleSearch()
}
function targetCollection() {
  return ['all', 'unfiled'].includes(collectionFilter.value) ? null : collectionFilter.value
}
function openDialog(mode, value = '', context = {}) {
  if (busy.value || (unavailable.value && mode !== 'about')) return
  const settings = {
    create: ['新建笔记', '笔记将保存到当前分类。标题可以稍后在编辑器中修改。', '笔记标题', '新建笔记'],
    collection: ['新建分类', '用分类整理课程、项目和日常记录。', '分类名称', '创建分类'],
    rename: ['重命名分类', '修改名称不会影响分类中的笔记。', '分类名称', '保存名称'],
    renameNote: ['重命名笔记', '仅修改笔记标题，保留正文内容。已有双链会按保存后的标题重新解析。', '新标题', '保存标题'],
    moveNote: ['移动笔记', '选择笔记所属的分类，正文内容会保留。', '', '移动笔记'],
    tags: ['管理标签', '添加或移除标签，点击保存后生效。', '', '保存标签'],
    deleteNote: ['删除这篇笔记？', '这是永久删除，当前版本没有回收站。建议先导出需要保留的内容。', '', '删除笔记'],
    deleteCollection: ['删除这个分类？', '该分类及子分类将被移除，其中的笔记会保留并移到“未分类”。', '', '删除分类'],
    about: ['关于 MarkdownNote', '', '', ''],
  }
  const [title, description, label, confirm] = settings[mode]
  previousFocus = document.activeElement
  const targetId = context.targetId || (['rename', 'deleteCollection'].includes(mode) ? collectionFilter.value : selectedId.value)
  const target = documents.value.find(item => item.id === targetId)
  dialog.value = { mode, value, title, description, label, confirm, error: '', targetId, targetTitle: context.targetTitle || target?.title || collections.value.find(item => item.id === targetId)?.name, collectionId: context.collectionId ?? targetCollection(), destination: target?.collectionId || '', tags: [], tagDraft: '', tagsLoading: mode === 'tags', tagsLoaded: false }
  nextTick(() => { dialogElement.value?.showModal(); dialogField.value?.focus() })
  if (mode === 'tags') void loadDialogTags(dialog.value)
}
async function loadDialogTags(current = dialog.value) {
  if (!current || current.mode !== 'tags' || busy.value) return
  current.tagsLoading = true
  current.tagsLoaded = false
  current.error = ''
  try {
    const tags = await notesApi.loadTags(current.targetId)
    if (disposed || dialog.value !== current) return
    current.tags = tags
    current.tagsLoaded = true
  } catch (error) {
    if (dialog.value === current) current.error = describeError(error)
  } finally {
    if (dialog.value === current) {
      current.tagsLoading = false
      await nextTick()
      if (dialog.value === current && current.tagsLoaded) dialogField.value?.focus()
    }
  }
}
function addDialogTag() {
  const current = dialog.value
  if (!current || current.mode !== 'tags' || !current.tagsLoaded || busy.value || !current.tagDraft.trim()) return
  try {
    current.tags = normalizeTags([...current.tags, current.tagDraft])
    current.tagDraft = ''
    current.error = ''
  } catch (error) { current.error = describeError(error) }
}
function closeDialog() {
  if (busy.value) return
  dialogElement.value?.close()
  dialog.value = null
  previousFocus?.focus?.()
}
async function submitDialog() {
  const current = dialog.value
  if (!current || busy.value) return
  if (current.mode === 'tags' && (!current.tagsLoaded || current.tagsLoading)) return
  const name = current.value.trim()
  if (current.label && (!name || unicodeLength(name) > 200 || /[\u0000-\u001f]/.test(name))) {
    current.error = '请输入 1～200 个字符，不能包含换行或控制字符。'
    return
  }
  busy.value = '正在保存操作…'
  try {
    await flushCurrent()
    if (current.mode === 'create') {
      const result = await notesApi.create(name, current.collectionId)
      const returned = normalizeDocuments({ roots: [result.snapshot] })
      const merged = new Map(documents.value.map((item) => [item.id, item]))
      for (const item of returned) merged.set(item.id, item)
      documents.value = [...merged.values()]
      query.value = ''
      tagFilter.value = ''
      selectedId.value = result.documentId
      dirty.value = false
      notify('笔记已创建，开始书写吧。')
    } else if (current.mode === 'collection') {
      const result = await notesApi.createCollection(name)
      collections.value.push(result)
      collectionFilter.value = result.id
    } else if (current.mode === 'rename') {
      const id = current.targetId
      await notesApi.renameCollection(id, name)
      collections.value.find((item) => item.id === id).name = name
    } else if (current.mode === 'renameNote') {
      const id = current.targetId
      if (id === selectedId.value) {
        if (!editor.value) throw new Error('编辑器尚未准备好，请稍后重试。')
        await editor.value.renameTitle(name)
      } else {
        const source = await notesApi.read(id)
        await notesApi.update({ documentId: id, title: name, bodyMd: source.bodyMd || '' })
        onSaved({ documentId: id, title: name })
      }
      notify('笔记已重命名。')
    } else if (current.mode === 'moveNote') {
      const doc = documents.value.find(item => item.id === current.targetId)
      if (!doc) throw new Error('笔记已不存在，请刷新列表。')
      const destination = current.destination || null
      if (destination && !collections.value.some(item => item.id === destination)) throw new Error('目标分类已不存在，请重新选择。')
      await notesApi.move(doc.id, destination)
      doc.collectionId = destination
      notify('笔记分类已更新。')
    } else if (current.mode === 'tags') {
      const tags = normalizeTags([...current.tags, ...(current.tagDraft.trim() ? [current.tagDraft] : [])])
      const saved = await notesApi.saveTags(current.targetId, tags)
      const doc = documents.value.find(item => item.id === current.targetId)
      if (doc) doc.tags = saved
      if (current.targetId === selectedId.value) {
        ++tagVersion
        noteTags.value = saved
        noteTagsError.value = ''
        tagsLoading.value = false
      }
      await refreshTags()
      notify('笔记标签已更新。')
    } else if (current.mode === 'deleteNote') {
      const id = current.targetId
      await notesApi.remove(id)
      documents.value = documents.value.filter((item) => item.id !== id)
      if (selectedId.value === id) { selectedId.value = ''; dirty.value = false }
      notify('笔记已删除。')
      void refreshTags()
    } else if (current.mode === 'deleteCollection') {
      const id = current.targetId
      await notesApi.deleteCollection(id)
      const deleted = new Set([id])
      let previousSize = 0
      while (previousSize !== deleted.size) {
        previousSize = deleted.size
        for (const item of collections.value) if (deleted.has(item.parentId)) deleted.add(item.id)
      }
      collections.value = collections.value.filter((item) => !deleted.has(item.id))
      for (const doc of documents.value) if (deleted.has(doc.collectionId)) doc.collectionId = null
      collectionFilter.value = 'unfiled'
      notify('分类已删除，笔记已移到未分类。')
    }
    scheduleSearch()
    busy.value = ''
    closeDialog()
  } catch (error) { current.error = describeError(error) }
  finally { busy.value = '' }
}
function createFromLink(title) {
  const hit = documents.value.find((item) => item.title.toLowerCase() === title.toLowerCase())
  if (hit) selectNote(hit.id)
  else openDialog('create', title)
}
function moveNote(event) {
  const collectionId = event.target.value || null
  const doc = selected.value
  event.target.value = doc?.collectionId || ''
  if (!doc || doc.collectionId === collectionId) return
  void action('正在移动笔记…', async () => {
    await flushCurrent()
    await notesApi.move(doc.id, collectionId)
    doc.collectionId = collectionId
    notify('笔记分类已更新。')
  })
}
function endItemDrag() { itemDrag.value = null; dropTarget.value = '' }
function blockExternalFileDrop(event) {
  if (!Array.from(event.dataTransfer?.types || []).includes('Files')) return
  event.preventDefault()
  event.stopPropagation()
  if (event.type === 'drop') notify('导入文件请使用左下角“导入 Markdown / 文本”，当前编辑内容会保留。', 'error')
}
function startNoteDrag(event, note) {
  if (busy.value || unavailable.value || dialog.value || !writeNoteDrag(event.dataTransfer, note)) { event.preventDefault(); return }
  closeMenu()
  itemDrag.value = { kind: 'note', id: note.id }
}
function startTagDrag(event, tag) {
  if (busy.value || unavailable.value || dialog.value || !writeTagDrag(event.dataTransfer, tag)) { event.preventDefault(); return }
  closeMenu()
  itemDrag.value = { kind: 'tag', name: tag }
}
function overDropTarget(event, kind, key) {
  if (busy.value || unavailable.value || dialog.value || dragKind(event.dataTransfer) !== kind) return
  event.preventDefault()
  event.dataTransfer.dropEffect = kind === 'note' ? 'move' : 'copy'
  dropTarget.value = key
}
function leaveDropTarget(event, key) {
  if (dropTarget.value === key && !event.currentTarget.contains(event.relatedTarget)) dropTarget.value = ''
}
function dropOnCollection(event, collectionId) {
  if (dragKind(event.dataTransfer) !== 'note') return
  event.preventDefault(); event.stopPropagation()
  const payload = readNoteDrag(event.dataTransfer)
  endItemDrag()
  void action('正在移动笔记…', async () => {
    if (!payload) throw new Error('无法识别拖拽的笔记，请从笔记列表重新拖动。')
    const note = documents.value.find(item => item.id === payload.id)
    if (!note) throw new Error('拖拽的笔记已不存在，请刷新列表。')
    if (collectionId && !collections.value.some(item => item.id === collectionId)) throw new Error('目标分类已不存在，请刷新列表。')
    if ((note.collectionId || null) === collectionId) { notify('笔记已在该分类中。'); return }
    await flushCurrent()
    await notesApi.move(note.id, collectionId)
    note.collectionId = collectionId
    scheduleSearch()
    notify(collectionId ? '笔记已移到目标分类。' : '笔记已移到未分类。')
  })
}
function syncSavedTags(documentId, tags) {
  const note = documents.value.find(item => item.id === documentId)
  if (note) note.tags = tags
  if (documentId === selectedId.value) {
    ++tagVersion
    noteTags.value = tags
    noteTagsError.value = ''
    tagsLoading.value = false
  }
}
function dropTagOnNote(event, documentId) {
  if (dragKind(event.dataTransfer) !== 'tag') return
  event.preventDefault(); event.stopPropagation()
  const payload = readTagDrag(event.dataTransfer)
  endItemDrag()
  void action('正在添加标签…', async () => {
    if (!payload || !documents.value.some(item => item.id === documentId)) throw new Error('无法识别标签或目标笔记，请刷新后重试。')
    await flushCurrent()
    const previous = await notesApi.loadTags(documentId)
    const tags = normalizeTags([...previous, payload.name])
    const saved = await notesApi.saveTags(documentId, tags)
    syncSavedTags(documentId, saved)
    await refreshTags()
    scheduleSearch()
    notify('标签已添加到目标笔记。')
  })
}
function insertDroppedLink({ documentId, position, append }) {
  endItemDrag()
  void action('正在插入双向链接…', async () => {
    const note = documents.value.find(item => item.id === documentId)
    if (!note || !editor.value) throw new Error('请打开目标笔记后，再从列表拖入另一篇笔记。')
    editor.value.insertWikiLink(note, position, append)
    await editor.value.flushSave()
    notify('双向链接已插入并保存。')
  })
}
function saveTags(tags) {
  void action('正在保存标签…', async () => {
    const normalized = normalizeTags(tags)
    noteTags.value = await notesApi.saveTags(selectedId.value, normalized)
    tagDraft.value = ''
    noteTagsError.value = ''
    await refreshTags()
    scheduleSearch()
  })
}
function importNotes() {
  void action('正在导入笔记…', async () => {
    await flushCurrent()
    const paths = await notesApi.chooseFiles()
    if (!paths.length) return
    const { accepted, rejected } = splitImportPaths(paths)
    if (!accepted.length) throw new Error('请选择 .md、.markdown、.txt 或 .text 文件。当前仅支持 Markdown 和纯文本文件。')
    const beforeIds = new Set(documents.value.map((item) => item.id))
    const snapshots = await notesApi.importFiles(accepted, targetCollection())
    const imported = normalizeDocuments({ roots: snapshots })
    const merged = new Map(documents.value.map((item) => [item.id, item]))
    for (const item of imported) merged.set(item.id, item)
    documents.value = [...merged.values()]
    scheduleSearch()
    const count = imported.filter((item) => !beforeIds.has(item.id)).length
    const skipped = rejected.length ? `已跳过 ${rejected.length} 个不支持的文件。` : ''
    if (count < accepted.length) {
      notify(`新增 ${count} 篇笔记，${accepted.length - count} 个文件未新增。请检查文件编码和访问权限，再单独重试。${skipped}`, 'error')
    } else notify(`已新增 ${count} 篇笔记。${skipped}`)
  })
}
function exportNote(documentId = selectedId.value) {
  if (!documentId) return
  void action('正在导出笔记…', async () => {
    await flushCurrent()
    if (documentId === selectedId.value && !editor.value) throw new Error('编辑器尚未准备好。')
    const snapshot = documentId === selectedId.value ? await editor.value.flushSave() : await notesApi.read(documentId)
    const path = await notesApi.exportFile(exportName(snapshot.title), markdownExport(snapshot))
    if (path) notify(`已导出：${path}`)
  })
}
function onBeforeUnload(event) {
  if (!dirty.value && !busy.value) return
  event.preventDefault()
  event.returnValue = ''
}
function onKeydown(event) {
  if (event.key === 'F11') { event.preventDefault(); closeMenu(); void toggleFullscreen(); return }
  if (!dialog.value && !contextMenu.value && event.key === 'Escape' && (fullscreen.value || focusMode.value)) {
    event.preventDefault()
    if (fullscreen.value) void toggleFullscreen()
    else focusMode.value = false
    return
  }
  if (!dialog.value && event.key === 'F8') { event.preventDefault(); if (!fullscreen.value) focusMode.value = !focusMode.value; closeMenu(); return }
  if (event.defaultPrevented || contextMenu.value || dialog.value) return
  if (!(event.ctrlKey || event.metaKey)) return
  if (event.key.toLowerCase() === 'k') {
    event.preventDefault()
    void (async () => { if (fullscreen.value) await toggleFullscreen(); focusMode.value = false; listHidden.value = false; await nextTick(); searchInput.value?.focus() })()
  }
  if (event.key.toLowerCase() === 'n' && !dialog.value) { event.preventDefault(); openDialog('create', '新建笔记') }
}
onMounted(async () => {
  document.title = 'MarkdownNote'
  window.addEventListener('beforeunload', onBeforeUnload)
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('pointerdown', dismissMenu, true)
  window.addEventListener('scroll', dismissMenu, true)
  window.addEventListener('resize', closeMenuOnWindowChange)
  window.addEventListener('blur', closeMenuOnWindowChange)
  await loadWorkspace()
  if (desktopAvailable && !disposed && !workspaceError.value) {
    try {
      const lastId = localStorage.getItem('course.lastNoteId')
      if (lastId && documents.value.some(item => item.id === lastId)) selectedId.value = lastId
      else if (lastId) localStorage.removeItem('course.lastNoteId')
    } catch { /* A missing/deleted note or unavailable preference does not block startup. */ }
  }
  if (!desktopAvailable || disposed) return
  try {
    stopIndexListener = await listen('markdownnote://document-index', ({ payload }) => {
      if (payload?.documentId === selectedId.value && ['indexed', 'ready'].includes(payload?.status)) void editor.value?.refreshLinks()
      if (filtered.value) scheduleSearch()
    })
    stopCloseListener = await getCurrentWindow().onCloseRequested(async (event) => {
      if (closeAllowed) return
      event.preventDefault()
      if (busy.value) { notify('当前操作尚未结束，请稍后关闭窗口。', 'error'); return }
      await action('正在保存并关闭…', async () => {
        await flushCurrent()
        closeAllowed = true
        try { await getCurrentWindow().destroy() }
        catch (error) { closeAllowed = false; throw error }
      })
    })
  } catch (error) { notify(`窗口关闭保护未能启用：${describeError(error)}。关闭前请手动保存。`, 'error') }
})
onBeforeUnmount(() => {
  disposed = true
  clearTimeout(searchTimer)
  clearTimeout(alertTimer)
  ++searchVersion
  ++tagVersion
  stopIndexListener?.()
  stopCloseListener?.()
  window.removeEventListener('beforeunload', onBeforeUnload)
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('pointerdown', dismissMenu, true)
  window.removeEventListener('scroll', dismissMenu, true)
  window.removeEventListener('resize', closeMenuOnWindowChange)
  window.removeEventListener('blur', closeMenuOnWindowChange)
})
function closeMenuOnWindowChange() { closeMenu() }
</script>

<template>
  <div class="course-app" :class="{ 'details-hidden': !detailsVisible, 'navigation-hidden': !navigationVisible, 'list-hidden': !listVisible, 'focus-mode': focused, 'layout-dragging': dragging }" :style="layoutStyle" @dragend="endItemDrag" @dragover.capture="blockExternalFileDrop" @drop.capture="blockExternalFileDrop">
    <aside v-show="navigationVisible" class="course-sidebar" aria-label="笔记导航">
      <div class="course-brand"><span class="course-brand-mark"><Icon name="book" :size="23" /></span><div><strong>MarkdownNote</strong><small>MARKDOWN 笔记</small></div></div>
      <button class="course-primary course-new" :disabled="unavailable || !!busy" @click="openDialog('create', '新建笔记')"><Icon name="plus" />新建笔记<kbd>Ctrl N</kbd></button>
      <nav class="course-nav">
        <button :class="{ active: collectionFilter === 'all' }" @click="collectionFilter = 'all'"><Icon name="book" />全部笔记<span>{{ documents.length }}</span></button>
        <button :class="{ active: collectionFilter === 'unfiled', 'course-drop-target': dropTarget === 'unfiled' }" @click="collectionFilter = 'unfiled'" @dragover="overDropTarget($event, 'note', 'unfiled')" @dragleave="leaveDropTarget($event, 'unfiled')" @drop="dropOnCollection($event, null)"><Icon name="file" />未分类<span>{{ documents.filter(item => !item.collectionId).length }}</span></button>
      </nav>
      <div class="course-sidebar-scroll">
        <div class="course-section-label"><span>我的分类</span><button class="course-icon-button" aria-label="新建分类" title="新建分类" :disabled="unavailable || !!busy" @click="openDialog('collection')"><Icon name="plus" :size="15" /></button></div>
        <nav class="course-nav course-collections">
          <button v-for="item in collectionOptions" :key="item.id" :class="{ active: collectionFilter === item.id, 'course-drop-target': dropTarget === 'collection:' + item.id }" :title="item.label + '（可拖入笔记）'" aria-haspopup="menu" @click="collectionFilter = item.id" @contextmenu="showMenu($event, 'collection', item)" @keydown.shift.f10.prevent="showMenu($event, 'collection', item)" @dragover="overDropTarget($event, 'note', 'collection:' + item.id)" @dragleave="leaveDropTarget($event, 'collection:' + item.id)" @drop="dropOnCollection($event, item.id)"><Icon name="folder" :size="17" /><span class="course-nav-label">{{ item.label }}</span><span>{{ documents.filter(doc => doc.collectionId === item.id).length }}</span></button>
        </nav>
        <p v-if="!collections.length" class="course-sidebar-hint">按课程或项目，给笔记一个位置。</p>
        <div class="course-section-label"><span>标签</span><button class="course-icon-button" aria-label="刷新标签" title="刷新标签" :disabled="!desktopAvailable || !!busy" @click="refreshTags"><Icon name="refresh" :size="14" /></button></div>
        <p v-if="tagsError" class="course-sidebar-hint course-warning">{{ tagsError }}</p>
        <p v-else-if="!allTags.length" class="course-sidebar-hint">右键笔记管理标签，也可在右侧添加。</p>
        <div class="course-tag-cloud"><button v-for="tag in allTags" :key="tag.name" class="course-chip" :class="{ active: tagFilter === tag.name }" :aria-pressed="tagFilter === tag.name" :draggable="!unavailable && !busy" title="拖到笔记可添加此标签" @dragstart="startTagDrag($event, tag.name)" @dragend="endItemDrag" @click="tagFilter = tagFilter === tag.name ? '' : tag.name"># {{ tag.name }}<span>{{ tag.noteCount }}</span></button></div>
      </div>
      <div class="course-sidebar-bottom">
        <button :disabled="unavailable || !!busy" @click="importNotes"><Icon name="upload" :size="17" />导入 Markdown / 文本</button>
        <div><span class="course-local-dot"></span><small>{{ desktopAvailable ? '本地笔记工作区' : '浏览器界面预览' }}</small><button class="course-icon-button" title="关于 MarkdownNote" aria-label="关于 MarkdownNote" :disabled="!!busy" @click="openDialog('about')"><Icon name="info" :size="17" /></button></div>
      </div>
      <div class="course-resizer" role="separator" tabindex="0" aria-label="调整导航栏宽度" aria-orientation="vertical" :aria-valuenow="Math.round(sidebarWidth)" :aria-valuemin="160" :aria-valuemax="Math.round(sidebarMax)" title="拖动调整宽度，双击恢复默认；左右方向键微调" @pointerdown="startResize($event, 'sidebar')" @pointermove="resize" @pointerup="endResize" @pointercancel="endResize" @lostpointercapture="endResize" @keydown="resizeKey($event, 'sidebar')" @dblclick="resetWidth('sidebar')"></div>
    </aside>

    <main class="course-main">
      <header class="course-topbar">
        <div class="course-breadcrumb"><Icon name="folder" :size="17" /><span>我的笔记</span><span>/</span><strong>{{ collectionName }}</strong></div>
        <div class="course-top-actions"><button class="course-icon-button" :aria-label="navigationVisible ? '收起导航栏' : '展开导航栏'" :title="navigationVisible ? '收起导航栏' : '展开导航栏'" :aria-expanded="navigationVisible" :disabled="focused" @click="sidebarHidden = !sidebarHidden"><Icon name="sidebar" /></button><button class="course-icon-button" :aria-label="listVisible ? '收起笔记列表' : '展开笔记列表'" :title="listVisible ? '收起笔记列表' : '展开笔记列表'" :aria-expanded="listVisible" :disabled="focused" @click="listHidden = !listHidden"><Icon name="list" /></button><button v-if="!fullscreen" class="course-focus-button" :class="{ active: focusMode }" :aria-label="focusMode ? '退出专注模式' : '进入专注模式'" :title="focusMode ? '退出专注模式（Esc / F8）' : '专注模式（F8）'" :aria-pressed="focusMode" @click="focusMode = !focusMode"><Icon name="focus" :size="16" /><span>{{ focusMode ? '退出专注' : '专注' }}</span></button><button class="course-icon-button" :aria-label="fullscreen ? '退出全屏' : '进入全屏'" :title="fullscreen ? '退出全屏（F11 / Esc）' : '全屏（F11）'" :aria-pressed="fullscreen" :disabled="changingFullscreen" @click="toggleFullscreen"><Icon :name="fullscreen ? 'shrink' : 'expand'" /></button><div class="course-theme-switch" role="group" aria-label="界面主题"><button :class="{ active: resolvedTheme === 'light' }" :aria-pressed="resolvedTheme === 'light'" aria-label="切换浅色主题" title="浅色主题" @click="setThemePreference('light')"><Icon name="sun" :size="14" /><span>浅色</span></button><button :class="{ active: resolvedTheme === 'dark' }" :aria-pressed="resolvedTheme === 'dark'" aria-label="切换深色主题" title="深色主题" @click="setThemePreference('dark')"><Icon name="moon" :size="14" /><span>深色</span></button></div><button class="course-icon-button" title="刷新工作区" aria-label="刷新工作区" :disabled="!desktopAvailable || !!busy" @click="reload"><Icon name="refresh" /></button><button class="course-icon-button" :class="{ active: detailOpen }" :aria-pressed="detailOpen" title="笔记信息" aria-label="笔记信息" @click="detailOpen = !detailOpen"><Icon name="info" /></button></div>
      </header>
      <div v-if="!desktopAvailable" class="course-preview-notice" role="status">界面预览 · 请使用桌面版新建、保存和导入本地笔记。这里不会生成演示数据。</div>
      <div v-if="alert" class="course-notification" :class="alert.type" :role="alert.type === 'error' ? 'alert' : 'status'"><span>{{ alert.text }}</span><button class="course-icon-button" aria-label="关闭提示" @click="alert = null"><Icon name="close" :size="16" /></button></div>
      <div class="course-workspace">
        <section v-show="listVisible" class="course-note-list" aria-label="笔记列表">
          <div class="course-list-heading"><div><h1>{{ collectionName }}</h1><span>{{ documents.length }} 篇本地笔记</span></div><div v-if="!['all', 'unfiled'].includes(collectionFilter)" class="course-list-tools"><button class="course-icon-button" title="重命名分类" aria-label="重命名分类" :disabled="!!busy" @click="openDialog('rename', collections.find(item => item.id === collectionFilter)?.name)"><Icon name="edit" :size="15" /></button><button class="course-icon-button" title="删除分类，保留笔记" aria-label="删除分类" :disabled="!!busy" @click="openDialog('deleteCollection')"><Icon name="trash" :size="15" /></button></div></div>
          <form class="course-search" role="search" @submit.prevent="runSearch"><Icon name="search" :size="17" /><label class="sr-only" for="note-search">搜索标题与正文</label><input id="note-search" ref="searchInput" v-model="query" placeholder="搜索标题与正文…" :disabled="unavailable" autocomplete="off" /><button v-if="query" class="course-icon-button" type="button" title="清除关键词" aria-label="清除关键词" @click="query = ''"><Icon name="close" :size="14" /></button><kbd v-else>Ctrl K</kbd></form>
          <div v-if="tagFilter" class="course-filter-row"><span># {{ tagFilter }}</span><button class="course-text-button" @click="tagFilter = ''">清除标签筛选</button></div>
          <div v-if="filtered" class="course-search-caption" role="status">{{ searching ? '正在搜索…' : searchError ? '搜索未完成' : `当前分类 ${listed.length} 条 · 全库匹配 ${resultTotal} 条` }}<span v-if="resultTruncated && !searching && !searchError">，仅取前 50 条，请缩小关键词</span></div>
          <div v-if="workspaceError" class="course-list-message" role="alert"><strong>工作区读取失败</strong><p>{{ workspaceError }}</p><button class="course-secondary" :disabled="!!busy" @click="reload">重新读取</button></div>
          <div v-else-if="initialLoading" class="course-list-message">正在读取本地笔记…</div>
          <div v-else-if="searchError" class="course-list-message" role="alert"><strong>搜索暂不可用</strong><p>{{ searchError }}</p><button class="course-secondary" @click="runSearch">重试搜索</button></div>
          <div v-else-if="searching" class="course-list-message">正在查找标题与正文…</div>
          <div v-else-if="!listed.length" class="course-list-message"><Icon :name="filtered ? 'search' : 'file'" :size="30" /><strong>{{ filtered ? '没有匹配的笔记' : '这里还没有笔记' }}</strong><p>{{ filtered ? '试试更短的关键词，或清除分类、标签筛选。' : '记录一个想法，或导入已有的 Markdown 文件。' }}</p><button v-if="!filtered" class="course-text-button" :disabled="unavailable || !!busy" @click="openDialog('create', '新建笔记')">新建第一篇笔记 →</button></div>
          <div v-else class="course-list-scroll"><button v-for="item in listed" :key="item.id" class="course-note-card" :class="{ active: selectedId === item.id, 'menu-target': contextMenu?.kind === 'note' && contextMenu.id === item.id, 'course-drop-target': dropTarget === 'note:' + item.id, 'course-drag-source': itemDrag?.kind === 'note' && itemDrag.id === item.id }" :aria-current="selectedId === item.id ? 'true' : undefined" aria-haspopup="menu" :disabled="!!busy" :draggable="!unavailable && !busy" title="拖到分类移动，拖到另一篇笔记正文插入双链" @dragstart="startNoteDrag($event, item)" @dragend="endItemDrag" @dragover="overDropTarget($event, 'tag', 'note:' + item.id)" @dragleave="leaveDropTarget($event, 'note:' + item.id)" @drop="dropTagOnNote($event, item.id)" @click="selectNote(item.id)" @contextmenu="showMenu($event, 'note', item)" @keydown.shift.f10.prevent="showMenu($event, 'note', item)"><div class="course-note-card-title"><Icon name="file" :size="16" /><strong><SearchHighlight :text="item.title" :query="query" /></strong><span v-if="selectedId === item.id && dirty" class="course-dirty-dot" title="有待保存的修改"></span></div><p v-if="item.snippet"><SearchHighlight :text="item.snippet" :query="query" /></p><p v-else>{{ item.collectionId ? collectionPath(collections, item.collectionId) : '未分类' }} · {{ item.contentType === 'text' ? '导入的文本' : 'Markdown 笔记' }}</p><div class="course-note-card-meta"><span>{{ item.contentType === 'text' ? 'TXT' : 'MD' }}</span><span v-for="tag in (item.tags || []).slice(0, 2)" :key="tag"># {{ tag }}</span><span v-if="selectedId === item.id" class="course-current-label">正在阅读</span></div></button></div>
          <div class="course-list-foot">{{ filtered ? '标题 / 正文 · 整段关键词匹配' : '右键笔记或分类，查看更多操作' }}</div>
          <div class="course-resizer" role="separator" tabindex="0" aria-label="调整笔记列表宽度" aria-orientation="vertical" :aria-valuenow="Math.round(listWidth)" :aria-valuemin="200" :aria-valuemax="Math.round(listMax)" title="拖动调整宽度，双击恢复默认；左右方向键微调" @pointerdown="startResize($event, 'list')" @pointermove="resize" @pointerup="endResize" @pointercancel="endResize" @lostpointercapture="endResize" @keydown="resizeKey($event, 'list')" @dblclick="resetWidth('list')"></div>
        </section>

        <section class="course-document-area" :inert="!!busy">
          <template v-if="selectedId">
            <div class="course-document-bar"><span><Icon name="file" :size="15" />{{ noteCollection }}</span><div><button class="course-text-button" :disabled="!!busy" @click="exportNote()"><Icon name="download" :size="15" />导出 .md</button><button class="course-icon-button course-danger-button" title="永久删除当前笔记" aria-label="删除当前笔记" :disabled="!!busy" @click="openDialog('deleteNote')"><Icon name="trash" :size="16" /></button></div></div>
            <NoteEditor :key="selectedId" ref="editor" :document-id="selectedId" :search-query="query" @dirty-change="dirty = $event" @saved="onSaved" @open-doc="selectNote" @create-from-link="createFromLink" @link-drop="insertDroppedLink" @notice="notify($event.text, $event.type)" />
          </template>
          <div v-else class="course-welcome"><div class="course-welcome-icon"><Icon name="book" :size="42" /></div><span class="course-eyebrow">留住想法，连接知识</span><h2>从一篇笔记开始。</h2><p>专注书写，用 Markdown 组织内容。<br />把课程、灵感和项目，慢慢连成自己的知识库。</p><button class="course-primary" :disabled="unavailable || !!busy" @click="openDialog('create', '新建笔记')"><Icon name="plus" />新建一篇笔记</button><div class="course-welcome-features"><span><Icon name="edit" :size="16" />源码与实时预览</span><span><Icon name="tag" :size="16" />分类与标签</span><span><Icon name="arrow" :size="16" />双向链接</span></div><small>选中左侧笔记，即可继续编辑</small></div>
        </section>

        <aside v-if="detailsVisible" class="course-details" aria-label="笔记信息">
          <div class="course-details-title"><strong>笔记信息</strong><button class="course-icon-button" title="收起笔记信息" aria-label="收起笔记信息" @click="detailOpen = false"><Icon name="close" :size="16" /></button></div>
          <template v-if="selected">
            <div class="course-detail-block"><label for="note-collection">所属分类</label><select id="note-collection" :value="selected.collectionId || ''" :disabled="!!busy" @change="moveNote"><option value="">未分类</option><option v-for="item in collectionOptions" :key="item.id" :value="item.id">{{ item.label }}</option></select></div>
            <div class="course-detail-block"><label for="note-tag-input">标签 <small>{{ noteTags.length }}/10</small></label><p v-if="tagsLoading" class="course-muted">正在读取标签…</p><div v-else-if="noteTagsError" class="course-detail-error" role="status">{{ noteTagsError }}<button class="course-text-button" :disabled="!!busy" @click="loadNoteTags">重试</button></div><template v-else><div class="course-note-tags"><button v-for="tag in noteTags" :key="tag" class="course-chip" :disabled="!!busy" :aria-label="'移除标签 ' + tag" :title="'移除标签：' + tag" @click="saveTags(noteTags.filter(item => item !== tag))"># {{ tag }}<Icon name="close" :size="12" /></button></div><form class="course-tag-form" @submit.prevent="tagDraft.trim() && saveTags([...noteTags, tagDraft])"><input id="note-tag-input" v-model="tagDraft" placeholder="添加标签，回车确认" :disabled="!!busy || noteTags.length >= 10" autocomplete="off" /><button class="course-icon-button" title="添加标签" aria-label="添加标签" :disabled="!!busy || !tagDraft.trim() || noteTags.length >= 10"><Icon name="plus" :size="16" /></button></form><small class="course-help">每个标签最多 20 字符，点击已有标签可移除。</small></template></div>
            <div class="course-detail-block course-properties"><span>文件类型 <strong>{{ selected.contentType === 'text' ? '纯文本' : 'Markdown' }}</strong></span><span>保存方式 <strong>自动保存</strong></span><span>正文编码 <strong>UTF-8</strong></span></div>
            <div class="course-writing-tip"><Icon name="info" :size="17" /><strong>让笔记彼此相连</strong><p>在正文输入 <code>[[另一篇笔记标题]]</code>，预览中点击即可跳转或创建目标笔记。</p></div>
          </template>
          <div v-else class="course-details-empty"><Icon name="file" :size="28" /><p>选择一篇笔记<br />查看分类与标签</p></div>
          <div class="course-details-bottom">记录想法 · 连接知识<br />本地保存，专注书写</div>
        </aside>
      </div>
      <footer class="course-statusbar" role="status"><span v-if="busy">{{ busy }}</span><span>{{ selectedId ? (dirty ? '修改待保存' : 'Markdown 笔记') : '个人笔记与知识管理' }}</span><span>Ctrl S 保存 · Ctrl K 搜索</span></footer>
    </main>

    <dialog ref="dialogElement" class="course-dialog" aria-labelledby="course-dialog-title" @cancel.prevent="closeDialog">
      <template v-if="dialog"><div class="course-dialog-heading"><h2 id="course-dialog-title">{{ dialog.title }}</h2><button class="course-icon-button" aria-label="关闭对话框" :disabled="!!busy" @click="closeDialog"><Icon name="close" /></button></div>
      <template v-if="dialog.mode === 'about'"><div class="course-about"><p><strong>MarkdownNote</strong> · 版本 0.3.0</p><p>本地 Markdown 笔记与知识管理工具。</p><p>用分类、标签和双向链接整理内容，专注记录与思考。</p><p><kbd>Ctrl N</kbd> 新建笔记　<kbd>Ctrl K</kbd> 搜索　<kbd>Ctrl S</kbd> 保存</p></div><div class="course-dialog-actions"><button class="course-primary" @click="closeDialog">知道了</button></div></template>
      <template v-else-if="dialog.mode === 'tags'">
        <p>{{ dialog.description }}</p><p class="course-delete-title">{{ dialog.targetTitle }}</p>
        <p v-if="dialog.tagsLoading" role="status">正在读取标签…</p>
        <template v-else-if="dialog.tagsLoaded">
          <div class="course-dialog-tags"><button v-for="tag in dialog.tags" :key="tag" class="course-chip" type="button" :disabled="!!busy" :aria-label="'移除标签 ' + tag" @click="dialog.tags = dialog.tags.filter(item => item !== tag); dialog.error = ''"># {{ tag }}<Icon name="close" :size="13" /></button><span v-if="!dialog.tags.length" class="course-muted">暂无标签</span></div>
          <form class="course-dialog-tag-form" @submit.prevent="addDialogTag"><label class="course-dialog-label" for="dialog-tag-input">添加标签（{{ dialog.tags.length }}/10）<input id="dialog-tag-input" ref="dialogField" v-model="dialog.tagDraft" :disabled="!!busy || dialog.tags.length >= 10" placeholder="输入标签，回车添加" autocomplete="off" /></label><button class="course-secondary" type="submit" :disabled="!!busy || !dialog.tagDraft.trim() || dialog.tags.length >= 10">添加</button></form>
          <small class="course-help">每个标签最多 20 字符，点击标签可移除。取消不会修改已有标签。</small>
        </template>
        <div v-if="dialog.error" class="course-inline-error" role="alert">{{ dialog.error }}</div>
        <button v-if="!dialog.tagsLoading && !dialog.tagsLoaded" class="course-text-button" :disabled="!!busy" @click="loadDialogTags()">重试读取标签</button>
        <div class="course-dialog-actions"><button class="course-secondary" type="button" :disabled="!!busy" @click="closeDialog">取消</button><button class="course-primary" type="button" :disabled="!!busy || dialog.tagsLoading || !dialog.tagsLoaded" @click="submitDialog">{{ busy ? '处理中…' : '保存标签' }}</button></div>
      </template>
      <form v-else @submit.prevent="submitDialog"><p>{{ dialog.description }}</p><p v-if="['deleteNote', 'deleteCollection', 'moveNote'].includes(dialog.mode)" class="course-delete-title">{{ dialog.targetTitle }}</p><label v-if="dialog.label" class="course-dialog-label">{{ dialog.label }}<input ref="dialogField" v-model="dialog.value" :disabled="!!busy" autocomplete="off" /></label><label v-if="dialog.mode === 'moveNote'" class="course-dialog-label">目标分类<select aria-label="目标分类" v-model="dialog.destination" :disabled="!!busy"><option value="">未分类</option><option v-for="item in collectionOptions" :key="item.id" :value="item.id">{{ item.label }}</option></select></label><div v-if="dialog.error" class="course-inline-error" role="alert">{{ dialog.error }}</div><div class="course-dialog-actions"><button class="course-secondary" type="button" :disabled="!!busy" @click="closeDialog">取消</button><button :class="dialog.mode.startsWith('delete') ? 'course-danger' : 'course-primary'" :disabled="!!busy">{{ busy ? '处理中…' : dialog.confirm }}</button></div></form></template>
    </dialog>
    <div v-if="contextMenu" ref="menuElement" class="course-context-menu" role="menu" :aria-label="contextMenu.kind === 'note' ? '笔记操作' : '分类操作'" :style="{ left: contextMenu.x + 'px', top: contextMenu.y + 'px' }" @keydown="menuKeydown" @contextmenu.prevent>
      <div class="course-menu-caption">{{ contextMenu.title }}</div>
      <template v-for="item in menuItems" :key="item.key"><div v-if="item.separator" class="course-menu-separator" role="separator"></div><button role="menuitem" tabindex="-1" :class="{ danger: item.danger }" @click="chooseMenu(item.key)"><Icon :name="item.icon" :size="16" /><span>{{ item.label }}</span></button></template>
      <div class="course-menu-hint">↑ ↓ 选择 · Enter 确认 · Esc 关闭</div>
    </div>
  </div>
</template>
