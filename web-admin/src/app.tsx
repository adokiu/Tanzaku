import { useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import AppLayout from '@/layouts/AppLayout'
import CertificatesPage from '@/pages/CertificatesPage'
import ClientsPage from '@/pages/ClientsPage'
import DomainWhitelistPage from '@/pages/DomainWhitelistPage'
import InitPage from '@/pages/InitPage'
import LoginPage from '@/pages/LoginPage'
import NodesPage from '@/pages/NodesPage'
import NodeSecurityEventsPage from '@/pages/NodeSecurityEventsPage'
import NodeSecurityPolicyPage from '@/pages/NodeSecurityPolicyPage'
import OrdersPage from '@/pages/OrdersPage'
import PaymentChannelsPage from '@/pages/PaymentChannelsPage'
import PlansPage from '@/pages/PlansPage'
import SettingsPage from '@/pages/SettingsPage'
import ThemesPage from '@/pages/ThemesPage'
import UsersPage from '@/pages/UsersPage'
import DashboardPage from '@/pages/DashboardPage'
import ResourcePage from '@/pages/ResourcePage'
import TunnelsPage from '@/pages/TunnelsPage'
import { FullPageLoader } from '@/components/FullPageLoader'
import { ToastHost } from '@/components/Toast'
import { useAuthStore } from '@/stores/auth'
import { useBrandingStore } from '@/stores/branding'
import { useThemeStore } from '@/stores/theme'
import './styles.css'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } })

export default function App() {
  const applyTheme = useThemeStore((state) => state.apply)
  const loadBranding = useBrandingStore((state) => state.load)
  useEffect(() => {
    applyTheme()
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', applyTheme)
    return () => media.removeEventListener('change', applyTheme)
  }, [applyTheme])
  useEffect(() => {
    void loadBranding()
  }, [loadBranding])

  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <AdminRoutes />
        <ToastHost />
      </BrowserRouter>
    </QueryClientProvider>
  )
}

function AdminRoutes() {
  const { t } = useTranslation()
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
  if (statusError) return <FullPageMessage>{t('errors.boardUnreachable')}</FullPageMessage>
  if (initialized === null) return <FullPageLoader />
  if (!initialized) return <Routes><Route path="*" element={<InitPage />} /></Routes>
  return (
    <Routes>
      <Route path="/login" element={<LoginPage audience="admin" />} />
      <Route element={<ProtectedLayout />}>
        <Route path="/" element={<Navigate to="/dashboard" replace />} />
        <Route path="/dashboard" element={<DashboardPage />} />
        <Route path="/nodes" element={<NodesPage />} />
        <Route path="/users" element={<UsersPage />} />
        <Route path="/plans" element={<PlansPage />} />
        <Route path="/orders" element={<OrdersPage />} />
        <Route path="/payments" element={<PaymentChannelsPage />} />
        <Route path="/clients" element={<ClientsPage audience="admin" />} />
        <Route path="/tunnels" element={<TunnelsPage />} />
        <Route path="/certificates" element={<CertificatesPage />} />
        <Route path="/security/domains" element={<DomainWhitelistPage />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="/themes" element={<ThemesPage />} />
        <Route path="/security" element={<NodeSecurityPolicyPage />} />
        <Route path="/security/events" element={<NodeSecurityEventsPage />} />
        <Route path="/audit-logs" element={<ResourcePage endpoint="/v1/admin/audit-logs" />} />
      </Route>
      <Route path="*" element={<Navigate to="/dashboard" replace />} />
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
  if (loading) return <FullPageLoader />
  if (!account || account.role !== 'admin') return <Navigate to="/login" replace />
  return <AppLayout />
}

function FullPageMessage({ children }: { children: ReactNode }) {
  return (
    <main className="ambient-surface min-h-screen flex items-center justify-center text-sm text-muted-foreground">
      {children}
    </main>
  )
}
