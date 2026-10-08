import { defineConfig, mergeConfig } from 'vite'
import shared from './vite.config'

export default mergeConfig(shared, defineConfig({
  server: {
    port: 5173,
    proxy: { '/api': { target: 'http://127.0.0.1:9000' } },
  },
}))
