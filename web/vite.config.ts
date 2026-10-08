import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { '@': resolve(root, 'src') } },
  build: {
    target: 'esnext',
    rollupOptions: { input: { admin: resolve(root, 'admin.html'), user: resolve(root, 'user.html') } },
  },
})
