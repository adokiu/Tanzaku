import { useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import apiClient from '@/api/client'
import AppLayout from '@/layouts/AppLayout'
import ClientsPage from '@/pages/ClientsPage'
import InitPage from '@/pages/InitPage'
import LoginPage from '@/pages/LoginPage'
import NodesPage from '@/pages/NodesPage'
import NodeGroupsPage from '@/pages/NodeGroupsPage'
import NodeSecurityPage from '@/pages/NodeSecurityPage'
import PlansPage from '@/pages/PlansPage'
import SettingsPage from '@/pages/SettingsPage'
import UsersPage from '@/pages/UsersPage'
import ResourcePage from '@/pages/ResourcePage'
import TunnelCreatePage from '@/pages/TunnelCreatePage'
import TunnelDetailPage from '@/pages/TunnelDetailPage'
import TunnelListPage from '@/pages/TunnelListPage'
import { useAuthStore } from '@/stores/auth'
import { useThemeStore } from '@/stores/theme'
import './styles.css'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } })
type Audience = 'admin' | 'user'

export default function App({ audience }: { audience: Audience }) {
  const applyTheme = useThemeStore((state) => state.apply)
  useEffect(() => {
    applyTheme()
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', applyTheme)
    return () => media.removeEventListener('change', applyTheme)
  }, [applyTheme])

  return <QueryClientProvider client={queryClient}><BrowserRouter><AppRoutes audience={audience} /></BrowserRouter></QueryClientProvider>
}

function AppRoutes({ audience }: { audience: Audience }) {
  if (audience === 'admin') return <AdminRoutes />
  return <UserRoutes />
}

function AdminRoutes() {
  const [initialized, setInitialized] = useState<boolean | null>(null)
  const [statusError, setStatusError] = useState(false)
  useEffect(() => {
    let active = true
    apiClient.get('/init/status').then((response) => {
      if (active) setInitialized(Boolean(response.data.initialized))
    }).catch(() => {
      if (active) setStatusError(true)
    })
    return () => { active = false }
  }, [])
  if (statusError) return <FullPageMessage>无法连接 board，请检查服务状态</FullPageMessage>
  if (initialized === null) return <FullPageMessage>正在检查初始化状态</FullPageMessage>
  if (!initialized) return <Routes><Route path="*" element={<InitPage />} /></Routes>
  return (
    <Routes>
      <Route path="/login" element={<LoginPage audience="admin" />} />
      <Route element={<ProtectedLayout audience="admin" />}>
        <Route path="/" element={<Navigate to="/dashboard" replace />} />
        <Route path="/dashboard" element={<ResourcePage title="系统概览" endpoint="/v1/admin/dashboard" />} />
        <Route path="/nodes" element={<NodesPage />} />
        <Route path="/node-groups" element={<NodeGroupsPage />} />
        <Route path="/users" element={<UsersPage />} />
        <Route path="/plans" element={<PlansPage />} />
        <Route path="/clients" element={<ClientsPage audience="admin" />} />
        <Route path="/tunnels" element={<ResourcePage title="隧道管理" endpoint="/v1/admin/tunnels" />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="/security" element={<NodeSecurityPage />} />
        <Route path="/certificates" element={<ResourcePage title="证书管理" endpoint="/v1/admin/certificates" />} />
        <Route path="/audit-logs" element={<ResourcePage title="审计日志" endpoint="/v1/admin/audit-logs" />} />
      </Route>
      <Route path="*" element={<Navigate to="/dashboard" replace />} />
    </Routes>
  )
}

function UserRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage audience="user" />} />
      <Route element={<ProtectedLayout audience="user" />}>
        <Route path="/" element={<Navigate to="/overview" replace />} />
        <Route path="/overview" element={<ResourcePage title="订阅与流量" endpoint="/v1/subscription" />} />
        <Route path="/clients" element={<ClientsPage audience="user" />} />
        <Route path="/tunnels/new" element={<TunnelCreatePage />} />
        <Route path="/tunnels/:tunnelId" element={<TunnelDetailPage />} />
        <Route path="/tunnels" element={<TunnelListPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/overview" replace />} />
    </Routes>
  )
}

function ProtectedLayout({ audience }: { audience: Audience }) {
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
  if (!account) return <Navigate to="/login" replace />
  if (audience === 'admin' && account.role !== 'admin') {
    return <Navigate to="/login" replace />
  }
  if (audience === 'user' && account.role !== 'user' && account.role !== 'admin') {
    return <Navigate to="/login" replace />
  }
  return <AppLayout audience={audience} />
}

function FullPageMessage({ children }: { children: ReactNode }) {
  return <main className="min-h-screen flex items-center justify-center bg-background text-sm text-muted-foreground">{children}</main>
}
