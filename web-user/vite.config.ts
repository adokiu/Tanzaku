import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { '@': resolve(root, 'src') } },
  server: {
    port: 5174,
    proxy: {
      '/api': { target: 'http://172.18.10.1:9001', changeOrigin: true },
    },
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
    rollupOptions: { input: { index: resolve(root, 'index.html') } },
  },
})
