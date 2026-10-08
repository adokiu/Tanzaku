import axios from 'axios'
import { useAuthStore } from '@/stores/auth'

const apiClient = axios.create({
  baseURL: '/api',
  timeout: 30000,
  withCredentials: true,
  headers: { 'Content-Type': 'application/json' },
})

apiClient.interceptors.request.use((config) => {
  const method = config.method?.toLowerCase() ?? 'get'
  if (method === 'get' || method === 'head' || method === 'options') {
    return config
  }
  const path = config.url ?? ''
  if (path.startsWith('/auth/') || path.startsWith('/init/')) {
    return config
  }
  const csrf = useAuthStore.getState().account?.csrf_token
  if (csrf) {
    config.headers.set('X-CSRF-Token', csrf)
  }
  return config
})

export default apiClient
