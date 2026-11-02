<script setup>
defineProps({ name: String, size: { type: Number, default: 18 } })
const paths = {
  book: 'M4 4h6c1.2 0 2 .8 2 2v14c0-1.2-.8-2-2-2H4z M20 4h-6c-1.2 0-2 .8-2 2v14c0-1.2.8-2 2-2h6z',
  plus: 'M12 5v14 M5 12h14',
  search: 'M21 21l-5-5 M18 10.5a7.5 7.5 0 1 1-15 0 7.5 7.5 0 0 1 15 0',
  folder: 'M3 7V5h6l2 2h10v13H3z',
  file: 'M6 3h8l4 4v14H6z M14 3v5h4 M9 12h6 M9 16h6',
  tag: 'M3 3h8l10 10-8 8L3 11z M7 7h.01',
  upload: 'M12 16V3 M7 8l5-5 5 5 M4 16v5h16v-5',
  download: 'M12 3v13 M7 11l5 5 5-5 M4 16v5h16v-5',
  trash: 'M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7',
  sun: 'M12 3V1 M12 23v-2 M3 12H1 M23 12h-2 M4 4l2 2 M18 18l2 2 M4 20l2-2 M18 6l2-2 M17 12a5 5 0 1 1-10 0 5 5 0 0 1 10 0',
  moon: 'M21 13a9 9 0 0 1-10-10 9 9 0 1 0 10 10',
  refresh: 'M20 11a8 8 0 1 0-2 6 M20 4v7h-7',
  info: 'M12 11v6 M12 7h.01 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0',
  close: 'M6 6l12 12 M6 18L18 6',
  check: 'M4 12l5 5L20 6',
  edit: 'M4 20l4-1L21 6l-4-4L4 15z M14 5l4 4',
  arrow: 'M4 12h16 M14 6l6 6-6 6',
  sidebar: 'M3 4h18v16H3z M9 4v16 M5 8h2 M5 12h2',
  list: 'M3 4h18v16H3z M11 4v16 M5 8h4 M5 12h4 M5 16h4',
  expand: 'M8 3H3v5 M16 3h5v5 M3 16v5h5 M21 16v5h-5',
  shrink: 'M3 8h5V3 M21 8h-5V3 M8 21v-5H3 M16 21v-5h5',
  focus: 'M8 3H4v4 M16 3h4v4 M4 17v4h4 M20 17v4h-4 M8 8h8v8H8z',
}
</script>
<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="paths[name] || paths.file" /></svg>
</template>
