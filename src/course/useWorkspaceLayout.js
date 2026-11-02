import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'

export function useWorkspaceLayout({ detailOpen, desktopAvailable, notify }) {
  let saved = {}
  try { const value = JSON.parse(localStorage.getItem('course.layout') || '{}'); if (value && typeof value === 'object') saved = value } catch { /* Default layout. */ }
  const sidebarHidden = ref(saved.sidebarHidden === true)
  const listHidden = ref(saved.listHidden === true)
  const sidebarSize = ref(Number.isFinite(saved.sidebarWidth) ? saved.sidebarWidth : null)
  const listSize = ref(Number.isFinite(saved.listWidth) ? saved.listWidth : null)
  if (typeof saved.detailOpen === 'boolean') detailOpen.value = saved.detailOpen
  const viewportWidth = ref(window.innerWidth)
  const focusMode = ref(false)
  const fullscreen = ref(false)
  const changingFullscreen = ref(false)
  const dragging = ref(false)
  const focused = computed(() => focusMode.value || fullscreen.value)
  const navigationVisible = computed(() => !sidebarHidden.value && !focused.value)
  const listVisible = computed(() => !listHidden.value && !focused.value)
  const detailsVisible = computed(() => detailOpen.value && !focused.value)
  const widthBudget = computed(() => Math.max(760, viewportWidth.value) - 280 - (detailsVisible.value && viewportWidth.value > 1080 ? (viewportWidth.value > 1250 ? 222 : 192) : 0))
  const sidebarMax = computed(() => Math.min(320, widthBudget.value - (listVisible.value ? 200 : 0)))
  const clamp = (value, minimum, maximum) => Math.max(minimum, Math.min(value, maximum))
  const sidebarWidth = computed(() => clamp(sidebarSize.value ?? (viewportWidth.value <= 1080 ? 194 : viewportWidth.value <= 1250 ? 204 : 228), 160, sidebarMax.value))
  const listMax = computed(() => Math.min(420, widthBudget.value - (navigationVisible.value ? sidebarWidth.value : 0)))
  const listWidth = computed(() => clamp(listSize.value ?? (viewportWidth.value <= 1250 ? 248 : 282), 200, listMax.value))
  const layoutStyle = computed(() => ({ '--course-sidebar-width': `${sidebarWidth.value}px`, '--course-list-width': `${listWidth.value}px` }))
  let drag
  let saveTimer
  let stopped = false
  function saveLayout() {
    try { localStorage.setItem('course.layout', JSON.stringify({ sidebarHidden: sidebarHidden.value, listHidden: listHidden.value, sidebarWidth: sidebarSize.value, listWidth: listSize.value, detailOpen: detailOpen.value })) } catch { /* Optional layout preference. */ }
  }
  watch([sidebarHidden, listHidden, sidebarSize, listSize, detailOpen], () => { clearTimeout(saveTimer); saveTimer = setTimeout(saveLayout, 200) })
  function setWidth(pane, value) {
    if (pane === 'sidebar') sidebarSize.value = clamp(value, 160, sidebarMax.value)
    else listSize.value = clamp(value, 200, listMax.value)
  }
  function startResize(event, pane) {
    if (event.button !== 0) return
    event.preventDefault()
    drag = { pane, x: event.clientX, width: pane === 'sidebar' ? sidebarWidth.value : listWidth.value, handle: event.currentTarget, pointerId: event.pointerId }
    drag.handle.setPointerCapture(event.pointerId)
    dragging.value = true
  }
  function resize(event) {
    if (!drag || event.pointerId !== drag.pointerId) return
    setWidth(drag.pane, drag.width + event.clientX - drag.x)
  }
  function endResize() {
    if (drag?.handle.hasPointerCapture(drag.pointerId)) drag.handle.releasePointerCapture(drag.pointerId)
    drag = null
    dragging.value = false
    saveLayout()
  }
  function resizeKey(event, pane) {
    const current = pane === 'sidebar' ? sidebarWidth.value : listWidth.value
    const minimum = pane === 'sidebar' ? 160 : 200
    const maximum = pane === 'sidebar' ? sidebarMax.value : listMax.value
    if (event.key === 'Home') setWidth(pane, minimum)
    else if (event.key === 'End') setWidth(pane, maximum)
    else if (event.key === 'ArrowLeft') setWidth(pane, current - (event.shiftKey ? 32 : 12))
    else if (event.key === 'ArrowRight') setWidth(pane, current + (event.shiftKey ? 32 : 12))
    else return
    event.preventDefault()
  }
  function resetWidth(pane) { if (pane === 'sidebar') sidebarSize.value = null; else listSize.value = null }
  async function toggleFullscreen() {
    if (changingFullscreen.value) return
    changingFullscreen.value = true
    try {
      if (desktopAvailable) {
        const currentWindow = getCurrentWindow()
        const entering = !(await currentWindow.isFullscreen())
        await currentWindow.setFullscreen(entering)
        fullscreen.value = entering
      } else if (document.fullscreenElement) { await document.exitFullscreen(); fullscreen.value = false }
      else { await document.documentElement.requestFullscreen(); fullscreen.value = true }
    } catch (error) { notify(`全屏切换失败：${error?.message || error}。仍可使用专注模式扩大编辑区。`, 'error') }
    finally { changingFullscreen.value = false }
  }
  function onResize() {
    viewportWidth.value = window.innerWidth
    endResize()
    if (desktopAvailable && !changingFullscreen.value) void getCurrentWindow().isFullscreen().then(value => { if (!stopped && !changingFullscreen.value) fullscreen.value = value }).catch(() => {})
  }
  function onFullscreenChange() { if (!desktopAvailable) fullscreen.value = !!document.fullscreenElement }
  onMounted(() => {
    window.addEventListener('resize', onResize)
    window.addEventListener('blur', endResize)
    window.addEventListener('beforeunload', saveLayout)
    document.addEventListener('fullscreenchange', onFullscreenChange)
  })
  onBeforeUnmount(() => {
    stopped = true
    clearTimeout(saveTimer)
    endResize()
    window.removeEventListener('resize', onResize)
    window.removeEventListener('blur', endResize)
    window.removeEventListener('beforeunload', saveLayout)
    document.removeEventListener('fullscreenchange', onFullscreenChange)
  })
  return { sidebarHidden, listHidden, focusMode, fullscreen, changingFullscreen, focused, navigationVisible, listVisible, detailsVisible, dragging, sidebarWidth, listWidth, sidebarMax, listMax, layoutStyle, startResize, resize, endResize, resizeKey, resetWidth, toggleFullscreen }
}
