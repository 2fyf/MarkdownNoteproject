// Theme preference: 'system' | 'light' | 'dark'. Dark is the default — this is
// a reading app, and a light window at night is the more disruptive wrong
// answer than a dark one at noon.
//
// Two values, not one. `preference` is what the user chose and is what the
// settings UI binds to; `resolved` is what is actually painted, and is the only
// thing the DOM ever sees. Keeping them apart is what lets "system" track the
// OS live instead of being snapshotted at startup.
//
// The resolved value is always written to the document as an explicit
// data-theme, so tokens.css needs exactly two blocks and no
// `@media (prefers-color-scheme)` branch to keep in sync with them.
//
// localStorage, not the SQLite app_settings table, and the reason is the same
// one persistedState.js gives: reading it is synchronous, so the very first
// paint already has the right theme. A round trip through Tauri IPC would
// guarantee a flash of the wrong one.

import { computed, ref, watch } from 'vue'

// Shared with the inline bootstrap in index.html. If you change it, change it
// there too — that script runs before any module and cannot import this.
export const THEME_STORAGE_KEY = 'markdownnote.theme'

export const THEME_OPTIONS = ['system', 'light', 'dark']

const DEFAULT_THEME = 'dark'

function readStoredPreference() {
  try {
    const raw = window.localStorage?.getItem(THEME_STORAGE_KEY)
    return THEME_OPTIONS.includes(raw) ? raw : DEFAULT_THEME
  } catch {
    // Private mode / disabled storage: fall back rather than break boot.
    return DEFAULT_THEME
  }
}

const darkQuery =
  typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)')
    : null

export const themePreference = ref(readStoredPreference())

// Bumped by the matchMedia listener so `resolvedTheme` recomputes when the OS
// flips while the preference is still 'system'.
const systemIsDark = ref(darkQuery ? darkQuery.matches : true)

export const resolvedTheme = computed(() => {
  if (themePreference.value === 'system') return systemIsDark.value ? 'dark' : 'light'
  return themePreference.value
})

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme
}

if (darkQuery) {
  const onChange = (event) => {
    systemIsDark.value = event.matches
  }
  // addEventListener is the modern API; addListener is the fallback for the
  // older WebKit that ships in some WKWebView builds.
  if (typeof darkQuery.addEventListener === 'function') darkQuery.addEventListener('change', onChange)
  else if (typeof darkQuery.addListener === 'function') darkQuery.addListener(onChange)
}

watch(
  resolvedTheme,
  (theme) => {
    applyTheme(theme)
  },
  { immediate: true },
)

watch(themePreference, (preference) => {
  try {
    window.localStorage?.setItem(THEME_STORAGE_KEY, preference)
  } catch {
    // Storage failures are not worth breaking a theme switch over; the choice
    // simply won't survive a restart.
  }
})

export function setThemePreference(preference) {
  if (!THEME_OPTIONS.includes(preference)) return
  themePreference.value = preference
}
