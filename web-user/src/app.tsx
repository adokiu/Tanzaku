import { useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import apiClient from '@/api/client'
import AppLayout from '@/layouts/AppLayout'
import ClientsPage from '@/pages/ClientsPage'
import LoginPage from '@/pages/LoginPage'
import PlansPage from '@/pages/PlansPage'
import ResourcePage from '@/pages/ResourcePage'
import TunnelCreatePage from '@/pages/TunnelCreatePage'
import TunnelDetailPage from '@/pages/TunnelDetailPage'
import TunnelListPage from '@/pages/TunnelListPage'
import { useAuthStore } from '@/stores/auth'
import { useThemeStore } from '@/stores/theme'
import './styles.css'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } })

export default function App() {
  const applyTheme = useThemeStore((state) => state.apply)
  useEffect(() => {
    applyTheme()
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', applyTheme)
    return () => media.removeEventListener('change', applyTheme)
  }, [applyTheme])

  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <UserRoutes />
      </BrowserRouter>
    </QueryClientProvider>
  )
}

function UserRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage audience="user" />} />
      <Route element={<ProtectedLayout />}>
        <Route path="/" element={<Navigate to="/overview" replace />} />
        <Route path="/overview" element={<ResourcePage title="订阅与流量" endpoint="/v1/subscription" />} />
        <Route path="/plans" element={<PlansPage />} />
        <Route path="/clients" element={<ClientsPage audience="user" />} />
        <Route path="/tunnels/new" element={<TunnelCreatePage />} />
        <Route path="/tunnels/:tunnelId" element={<TunnelDetailPage />} />
        <Route path="/tunnels" element={<TunnelListPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/overview" replace />} />
    </Routes>
  )
}

function ProtectedLayout() {
  const account = useAuthStore((state) => state.account)
  const setAccount = useAuthStore((state) => state.setAccount)
  const [loading, setLoading] = useState(true)
  useEffect(() => {
    let active = true
    apiClient.get('/auth/me').then((response) => {
      if (active) {
        const data = response.data as { user_id: string; email: string; role: 'admin' | 'user'; csrf_token?: string }
        setAccount({
          user_id: data.user_id,
          email: data.email,
          role: data.role,
          csrf_token: data.csrf_token ?? '',
        })
      }
    }).catch(() => {
      if (active) setAccount(null)
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [setAccount])
  if (loading) return <FullPageMessage>正在验证登录状态</FullPageMessage>
  if (!account || account.role !== 'user') return <Navigate to="/login" replace />
  return <AppLayout audience="user" />
}

function FullPageMessage({ children }: { children: ReactNode }) {
  return <main className="min-h-screen flex items-center justify-center bg-background text-sm text-muted-foreground">{children}</main>
}
