import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const host = process.env.MARKDOWNNOTE_DEV_HOST || '127.0.0.1'
const port = Number.parseInt(process.env.MARKDOWNNOTE_DEV_PORT || '1420', 10)

export default defineConfig({
  plugins: [vue()],
  optimizeDeps: { entries: ['index.html'] },
  server: {
    host,
    port: Number.isInteger(port) ? port : 1420,
    strictPort: process.env.MARKDOWNNOTE_STRICT_PORT === '1',
    watch: { ignored: ['**/src-tauri/target/**', '**/.local-data/**', '**/.local-logs/**'] },
  },
  clearScreen: false,
})
