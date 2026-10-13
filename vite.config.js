import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  // Vite не должен затирать ошибки компиляции Rust в общем окне `tauri dev`.
  clearScreen: false,
  server: {
    // 1420 занят Snipcast: два проекта на Tauri должны уживаться на одной машине.
    port: 1430,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**', '**/dist-release/**'] },
  },
})
