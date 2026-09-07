import { fileURLToPath, URL } from 'node:url'
import react from '@vitejs/plugin-react'
// `vitest/config` re-exports Vite's defineConfig with the `test` key typed.
import { defineConfig } from 'vitest/config'

// Tauri drives the dev server; the port is fixed because tauri.conf.json points at it.
const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  // Tauri expects a fixed port and treats a failure to bind as fatal.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    ...(host ? { hmr: { protocol: 'ws', host, port: 1421 } } : {}),
    watch: { ignored: ['**/src-tauri/**', '**/target/**', '**/reference/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // Both shipped platforms run a modern webview; match Tauri's documented targets.
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // Vite 8 minifies with Oxc; naming esbuild would require installing it separately.
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
