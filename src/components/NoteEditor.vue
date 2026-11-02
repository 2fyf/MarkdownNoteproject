<script setup>
import { ref, computed, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { EditorState, StateEffect, StateField } from '@codemirror/state'
import { EditorView, Decoration, keymap, lineNumbers, highlightActiveLine } from '@codemirror/view'
import { defaultKeymap, history, historyKeymap, indentWithTab, redo, isolateHistory } from '@codemirror/commands'
import { markdown } from '@codemirror/lang-markdown'
import { syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language'
import MarkdownText from './MarkdownText.vue'
import { call } from '../course/notesApi.js'
import { createSaveQueue } from '../editor/saveQueue.js'
import { markdownActions, markdownTransaction } from '../editor/markdownActions.js'
import { normalizeSearchQuery, searchMatchRanges } from '../course/searchHighlight.js'
import { dragKind, readNoteDrag, wikiLinkForNote } from '../course/noteDrag.js'

const props = defineProps({ documentId: { type: String, required: true }, searchQuery: { type: String, default: '' } })
const emit = defineEmits(['saved', 'dirty-change', 'open-doc', 'create-from-link', 'link-drop', 'notice'])
const title = ref('')
const body = ref('')
const baseline = ref({ title: '', bodyMd: '' })
const loading = ref(true)
const loaded = ref(false)
const saving = ref(false)
const saveError = ref('')
const loadError = ref('')
const linkError = ref('')
const openingLink = ref(false)
const linkDropActive = ref(false)
const links = ref({ outbound: [], backlinks: [] })
const host = ref(null)
const previewElement = ref(null)
const mode = ref('split')
const dirty = computed(() => loaded.value && (title.value !== baseline.value.title || body.value !== baseline.value.bodyMd))
const characters = computed(() => Array.from(body.value).length)
let view = null
let timer = null
let disposed = false
let linkVersion = 0
let positionTimer
let restoreFrame
const POSITION_KEY = 'course.editorPositions'
const setSearchQuery = StateEffect.define()
function sourceSearchMarks(doc, query) {
  return Decoration.set(searchMatchRanges(doc.toString(), query).map(({ from, to }) => Decoration.mark({ class: 'course-search-match' }).range(from, to)))
}
const searchHighlights = StateField.define({
  create(state) {
    const query = normalizeSearchQuery(props.searchQuery)
    return { query, marks: sourceSearchMarks(state.doc, query) }
  },
  update(value, transaction) {
    let query = value.query
    for (const effect of transaction.effects) if (effect.is(setSearchQuery)) query = effect.value
    return transaction.docChanged || query !== value.query ? { query, marks: sourceSearchMarks(transaction.state.doc, query) } : value
  },
  provide: field => EditorView.decorations.from(field, value => value.marks),
})
function highlightCurrentSearch() {
  const query = normalizeSearchQuery(props.searchQuery)
  if (view) {
    const first = searchMatchRanges(view.state.doc.toString(), query)[0]
    view.dispatch({ effects: [setSearchQuery.of(query), ...(first && mode.value !== 'preview' ? [EditorView.scrollIntoView(first.from, { y: 'center' })] : [])] })
  }
  nextTick(() => {
    if (query && mode.value !== 'edit') previewElement.value?.querySelector('mark.course-search-match')?.scrollIntoView({ block: 'center' })
  })
}
watch([() => props.searchQuery, mode], highlightCurrentSearch)

function storedPositions() {
  try { const entries = JSON.parse(localStorage.getItem(POSITION_KEY) || '[]'); return Array.isArray(entries) ? entries : [] }
  catch { return [] }
}
function savePosition() {
  if (!view || !loaded.value) return
  const entries = storedPositions()
  const previous = entries.find(item => item?.documentId === props.documentId)
  const { anchor, head } = view.state.selection.main
  const snapshot = { documentId: props.documentId, anchor, head, sourceTop: mode.value === 'preview' ? previous?.sourceTop || 0 : view.scrollDOM.scrollTop, previewTop: mode.value === 'edit' ? previous?.previewTop || 0 : previewElement.value?.scrollTop || 0 }
  try { localStorage.setItem(POSITION_KEY, JSON.stringify([snapshot, ...entries.filter(item => item?.documentId && item.documentId !== props.documentId)].slice(0, 20))) }
  catch { /* Position preferences are optional; note saving must still work. */ }
}
function schedulePosition() { clearTimeout(positionTimer); positionTimer = setTimeout(savePosition, 300) }
function restorePosition() {
  const saved = storedPositions().find(item => item?.documentId === props.documentId)
  if (!saved || !view) return
  const clamp = value => Number.isInteger(value) ? Math.max(0, Math.min(value, view.state.doc.length)) : 0
  view.dispatch({ selection: { anchor: clamp(saved.anchor), head: clamp(saved.head) } })
  restoreFrame = requestAnimationFrame(() => {
    restoreFrame = requestAnimationFrame(() => {
      if (!view || disposed) return
      if (Number.isFinite(saved.sourceTop)) view.scrollDOM.scrollTop = Math.max(0, saved.sourceTop)
      if (previewElement.value && Number.isFinite(saved.previewTop)) previewElement.value.scrollTop = Math.max(0, saved.previewTop)
    })
  })
}
function format(action) {
  if (!view || !loaded.value) return false
  if (mode.value === 'preview') mode.value = 'split'
  const transaction = markdownTransaction(view.state, action)
  if (!transaction) return false
  view.dispatch(transaction)
  nextTick(() => view?.focus())
  return true
}
function linkDragOver(event) {
  if (!loaded.value || dragKind(event.dataTransfer) !== 'note') return false
  event.preventDefault()
  event.dataTransfer.dropEffect = 'copy'
  linkDropActive.value = true
  return true
}
function leaveLinkDrop(event) {
  if (!event.currentTarget?.contains(event.relatedTarget)) linkDropActive.value = false
}
function endLinkDrag() { linkDropActive.value = false }
function dropNoteLink(event) {
  if (dragKind(event.dataTransfer) !== 'note') return false
  event.preventDefault()
  event.stopPropagation()
  linkDropActive.value = false
  const note = readNoteDrag(event.dataTransfer)
  if (!note || !view || !loaded.value) return true
  const inSource = mode.value !== 'preview' && event.target?.closest?.('.course-source-pane')
  const position = inSource ? view.posAtCoords({ x: event.clientX, y: event.clientY }, false) : null
  emit('link-drop', { documentId: note.id, position: position ?? view.state.doc.length, append: !inSource })
  return true
}
function insertWikiLink(note, position, append = false) {
  if (!view || !loaded.value) throw new Error('请先打开一篇笔记，再拖入另一篇笔记建立链接。')
  const link = wikiLinkForNote(note, props.documentId)
  const from = Math.max(0, Math.min(Number.isInteger(position) ? position : view.state.selection.main.head, view.state.doc.length))
  const insert = `${append && from > 0 ? '\n\n' : ''}${link}`
  mode.value = mode.value === 'preview' ? 'split' : mode.value
  view.dispatch({ changes: { from, insert }, selection: { anchor: from + insert.length }, scrollIntoView: true, annotations: isolateHistory.of('full') })
  nextTick(() => view?.focus())
}

try {
  const stored = localStorage.getItem('course.editorMode')
  if (['edit', 'split', 'preview'].includes(stored)) mode.value = stored
} catch { /* Optional preference only. */ }
watch(mode, (value) => { try { localStorage.setItem('course.editorMode', value) } catch { /* Optional. */ } })
watch(dirty, (value) => emit('dirty-change', value), { flush: 'sync' })

const queue = createSaveQueue({
  read: () => ({ documentId: props.documentId, title: title.value, bodyMd: body.value }),
  isDirty: () => dirty.value,
  write: (input) => call('update_note_source', { input }),
  acknowledge: (snapshot) => {
    baseline.value = { title: snapshot.title, bodyMd: snapshot.bodyMd }
    emit('saved', { documentId: snapshot.documentId, title: snapshot.title || '无标题笔记' })
  },
})

async function flushSave() {
  clearTimeout(timer)
  if (!loaded.value) throw new Error('笔记尚未成功读取，请重试后再操作。')
  saving.value = true
  saveError.value = ''
  try {
    await queue.flush()
    void loadLinks()
    return { documentId: props.documentId, title: title.value, bodyMd: body.value }
  } catch (error) {
    saveError.value = error.message
    throw error
  } finally {
    saving.value = false
    savePosition()
  }
}
async function renameTitle(value) {
  if (!loaded.value) throw new Error('笔记尚未成功读取，请稍后重试。')
  title.value = value
  await flushSave()
}
function saveInBackground() { void flushSave().catch(() => {}) }
watch([title, body], () => {
  if (!dirty.value) return
  clearTimeout(timer)
  timer = setTimeout(saveInBackground, 900)
})

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    const data = await call('load_note_source', { documentId: props.documentId })
    if (disposed) return
    title.value = data.title || ''
    body.value = data.bodyMd || ''
    baseline.value = { title: title.value, bodyMd: body.value }
    loaded.value = true
    await nextTick()
    buildEditor()
    void loadLinks()
  } catch (error) { loadError.value = error.message }
  finally { loading.value = false }
}

async function loadLinks() {
  const version = ++linkVersion
  try {
    const result = await call('load_note_links', { documentId: props.documentId })
    if (disposed || version !== linkVersion) return
    links.value = { outbound: result?.outbound || [], backlinks: result?.backlinks || [] }
    linkError.value = ''
  } catch (error) { if (!disposed && version === linkVersion) linkError.value = error.message }
}

function buildEditor() {
  if (!host.value || view) return
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: body.value,
      extensions: [
        lineNumbers(), history(), highlightActiveLine(), searchHighlights,
        EditorView.domEventHandlers({ dragover: linkDragOver, drop: dropNoteLink }),
        keymap.of([{ key: 'Mod-s', run: () => { saveInBackground(); return true } }, { key: 'Mod-Shift-z', run: redo, preventDefault: true }, ...markdownActions.map(item => ({ key: item.key, run: () => format(item.id) })), ...defaultKeymap, ...historyKeymap, indentWithTab]),
        markdown(), syntaxHighlighting(defaultHighlightStyle), EditorView.lineWrapping,
        EditorView.updateListener.of((update) => { if (update.docChanged) body.value = update.state.doc.toString(); if (update.docChanged || update.selectionSet) schedulePosition() }),
        EditorView.theme({
          '&': { height: '100%', color: 'var(--ink)', backgroundColor: 'transparent' },
          '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.85' },
          '.cm-content': { padding: '24px 0', caretColor: 'var(--accent)' },
          '.cm-gutters': { backgroundColor: 'transparent', border: 'none', color: 'var(--ink-3)' },
          '.cm-line': { paddingLeft: '12px', paddingRight: '20px' },
          '.cm-activeLine': { backgroundColor: 'var(--surface-hover)' },
          '.cm-cursor': { borderLeftColor: 'var(--accent)' },
          '&.cm-focused': { outline: 'none' },
        }),
        EditorView.contentAttributes.of({ 'aria-label': 'Markdown 正文', spellcheck: 'false' }),
      ],
    }),
  })
  view.scrollDOM.addEventListener('scroll', schedulePosition, { passive: true })
  if (normalizeSearchQuery(props.searchQuery)) highlightCurrentSearch()
  else restorePosition()
}

const previewBody = computed(() => (body.value || '*从左侧开始书写，这里会实时显示排版。*').replace(/\[\[([^\]\n]+)\]\]/g, (_, raw) => {
  const [target, ...alias] = raw.split('|')
  const label = (alias.join('|') || target).replace(/([\[\]\\])/g, '\\$1')
  return `[${label}](#wiki:${encodeURIComponent(target.trim())})`
}))
function navigateWiki(rawTitle) {
  const target = String(rawTitle).trim()
  if (!target) return
  const link = links.value.outbound.find((item) => item.title.toLowerCase() === target.toLowerCase())
  if (link?.documentId) emit('open-doc', link.documentId)
  else emit('create-from-link', target)
}
async function previewClick(event) {
  if (event.type === 'auxclick' && event.button !== 1) return
  const anchor = event.target?.closest?.('a')
  if (!anchor) return
  const href = anchor?.getAttribute('href') || ''
  event.preventDefault()
  if (href.startsWith('#wiki:')) {
    try { navigateWiki(decodeURIComponent(href.slice(6))) } catch { linkError.value = '双链地址格式不正确。' }
    return
  }
  if (href.startsWith('#')) {
    let target
    try { target = decodeURIComponent(href.slice(1)) } catch { target = '' }
    const heading = [...(previewElement.value?.querySelectorAll('[id]') || [])].find(item => item.id === target)
    if (heading) heading.scrollIntoView({ block: 'start' })
    else emit('notice', { text: '未找到该链接对应的正文标题。', type: 'error' })
    return
  }
  if (!/^https?:\/\//i.test(href)) {
    emit('notice', { text: '此链接不是完整网页地址。网页请使用 https:// 地址，笔记请使用 [[笔记标题]]。', type: 'error' })
    return
  }
  if (openingLink.value) return
  openingLink.value = true
  try { await call('open_external_url', { url: href }) }
  catch (error) { emit('notice', { text: `打开链接失败：${error.message}`, type: 'error' }) }
  finally { openingLink.value = false }
}
function onKeydown(event) {
  if (event.defaultPrevented) return
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
    event.preventDefault()
    if (loaded.value) saveInBackground()
  }
}
onMounted(() => { void load(); window.addEventListener('keydown', onKeydown); window.addEventListener('pagehide', savePosition); window.addEventListener('beforeunload', savePosition); window.addEventListener('dragend', endLinkDrag) })
onBeforeUnmount(() => {
  savePosition()
  disposed = true
  clearTimeout(timer)
  clearTimeout(positionTimer)
  cancelAnimationFrame(restoreFrame)
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('pagehide', savePosition)
  window.removeEventListener('beforeunload', savePosition)
  window.removeEventListener('dragend', endLinkDrag)
  view?.scrollDOM.removeEventListener('scroll', schedulePosition)
  view?.destroy()
})
defineExpose({ flushSave, renameTitle, refreshLinks: loadLinks, insertWikiLink })
</script>

<template>
  <section class="course-editor" aria-label="笔记编辑器" :aria-busy="loading">
    <div v-if="loadError" class="course-inline-error" role="alert">读取失败：{{ loadError }} <button @click="load">重新读取</button></div>
    <div v-if="loading" class="course-loading">正在读取笔记…</div>
    <template v-else-if="loaded">
      <div class="course-title-row">
        <label class="sr-only" for="note-title">笔记标题</label>
        <input id="note-title" v-model="title" placeholder="无标题笔记" autocomplete="off" @keydown.enter.prevent="view?.focus()" />
      </div>
      <div class="course-editor-toolbar">
        <div class="course-segment" role="group" aria-label="编辑视图">
          <button v-for="item in [{ id: 'edit', label: '源码' }, { id: 'split', label: '分屏' }, { id: 'preview', label: '预览' }]" :key="item.id" :aria-pressed="mode === item.id" :class="{ active: mode === item.id }" @click="mode = item.id">{{ item.label }}</button>
        </div>
        <span class="course-save-state" :class="{ failed: saveError }" role="status">{{ saveError ? '保存失败 · 修改仍在编辑器中' : saving ? '正在保存…' : dirty ? '有待保存的修改' : '已保存到本地' }}</span>
        <button class="course-text-button" :disabled="saving" title="Ctrl+S" @click="saveInBackground">{{ saveError ? '重试保存' : '保存' }}</button>
      </div>
      <div class="course-format-toolbar" role="toolbar" aria-label="Markdown 格式工具栏"><button v-for="item in markdownActions" :key="item.id" type="button" :class="'format-' + item.id" :aria-label="item.label" :title="item.label + ' (' + item.hint + ')'" @mousedown.prevent @click="format(item.id)">{{ item.text }}</button><span>选中文字，快速排版</span></div>
      <div v-if="saveError" class="course-inline-error" role="alert">{{ saveError }}。请保留此窗口并重试保存。</div>
    </template>
    <div class="course-editor-panes" :class="[`mode-${mode}`, { 'course-link-drop-target': linkDropActive }]" v-show="loaded && !loading" @dragover="linkDragOver" @dragleave="leaveLinkDrop" @drop="dropNoteLink" @dragend="linkDropActive = false">
      <div v-show="mode !== 'preview'" class="course-source-pane"><div class="course-pane-caption">MARKDOWN</div><div ref="host" class="course-cm-host"></div></div>
      <div v-show="mode !== 'edit'" class="course-preview-pane"><div class="course-pane-caption">预览</div><div ref="previewElement" class="course-rendered" @scroll.passive="schedulePosition" @click="previewClick" @auxclick="previewClick"><MarkdownText :text="previewBody" :highlight-query="searchQuery" /></div></div>
    </div>
    <div v-if="loaded" class="course-editor-foot">
      <span>{{ characters.toLocaleString() }} 字符</span><span>Markdown · UTF-8</span><span>停顿 0.9 秒自动保存</span>
    </div>
    <footer v-if="loaded" class="course-links">
      <span class="course-link-label">双向链接</span>
      <span v-if="linkError" class="course-muted">链接读取失败 <button class="course-text-button" @click="loadLinks">重试</button></span>
      <template v-else>
        <button v-for="item in links.backlinks" :key="`back-${item.documentId}`" class="course-chip" :title="'来自：' + item.title" @click="emit('open-doc', item.documentId)">↗ {{ item.title }}</button>
        <button v-for="(item, index) in links.outbound" :key="`out-${index}`" class="course-chip" :class="{ unresolved: !item.documentId }" :title="item.documentId ? '打开引用笔记' : '创建链接目标笔记'" @click="navigateWiki(item.title)">[[{{ item.title }}]]{{ item.documentId ? '' : ' +' }}</button>
        <span v-if="!links.backlinks.length && !links.outbound.length" class="course-muted">输入 [[笔记标题]] 连接另一篇笔记；保存并完成本地索引后更新。</span>
      </template>
    </footer>
  </section>
</template>
