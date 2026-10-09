import { useEffect, useState } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import apiClient from '@/api/client'
import AppLayout from '@/layouts/AppLayout'
import AccountSecurityPage from '@/pages/AccountSecurityPage'
import CertificatesPage from '@/pages/CertificatesPage'
import ClientsPage from '@/pages/ClientsPage'
import DashboardPage from '@/pages/DashboardPage'
import LedgerPage from '@/pages/LedgerPage'
import LoginPage from '@/pages/LoginPage'
import NodeSecurityEventsPage from '@/pages/NodeSecurityEventsPage'
import OrderPayPage from '@/pages/OrderPayPage'
import OrdersPage from '@/pages/OrdersPage'
import PlansPage from '@/pages/PlansPage'
import ResourcePage from '@/pages/ResourcePage'
import TunnelsPage from '@/pages/TunnelsPage'
import { FullPageLoader } from '@/components/FullPageLoader'
import { ThemeBackground } from '@/components/ThemeBackground'
import { ToastHost } from '@/components/Toast'
import { useAuthStore } from '@/stores/auth'
import { useBrandingStore } from '@/stores/branding'
import { useThemeStore } from '@/stores/theme'
import { useThemeSettingsStore } from '@/stores/themeSettings'
import './styles.css'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } })

const THEME_SHORT_KEY = 'tanzaku-user-theme-short'
const THEME_VERSION_KEY = 'tanzaku-user-theme-version'

export default function App() {
  const applyTheme = useThemeStore((state) => state.apply)
  const loadBranding = useBrandingStore((state) => state.load)
  const loadThemeSettings = useThemeSettingsStore((state) => state.load)

  useEffect(() => {
    applyTheme()
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', applyTheme)
    return () => media.removeEventListener('change', applyTheme)
  }, [applyTheme])

  useEffect(() => {
    void loadBranding()
  }, [loadBranding])

  useEffect(() => {
    let active = true
    async function syncTheme() {
      try {
        const { short, version } = await loadThemeSettings()
        const prevShort = sessionStorage.getItem(THEME_SHORT_KEY)
        const prevVersion = sessionStorage.getItem(THEME_VERSION_KEY)
        if (prevShort !== null && prevVersion !== null && (prevShort !== short || prevVersion !== version)) {
          sessionStorage.setItem(THEME_SHORT_KEY, short)
          sessionStorage.setItem(THEME_VERSION_KEY, version)
          window.location.reload()
          return
        }
        sessionStorage.setItem(THEME_SHORT_KEY, short)
        sessionStorage.setItem(THEME_VERSION_KEY, version)
      } catch {
        // ignore
      }
    }
    void syncTheme()
    const timer = window.setInterval(() => {
      if (active) void syncTheme()
    }, 15_000)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [loadThemeSettings])

  return (
    <QueryClientProvider client={queryClient}>
      <ThemeBackground />
      <BrowserRouter>
        <UserRoutes />
        <ToastHost />
      </BrowserRouter>
    </QueryClientProvider>
  )
}

function UserRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage audience="user" />} />
      <Route element={<ProtectedLayout />}>
        <Route path="/" element={<Navigate to="/dashboard" replace />} />
        <Route path="/dashboard" element={<DashboardPage />} />
        <Route path="/overview" element={<Navigate to="/dashboard" replace />} />
        <Route path="/plans" element={<PlansPage />} />
        <Route path="/clients" element={<ClientsPage audience="user" />} />
        <Route path="/tunnels" element={<TunnelsPage />} />
        <Route path="/certificates" element={<CertificatesPage />} />
        <Route path="/orders" element={<OrdersPage />} />
        <Route path="/orders/:id/pay" element={<OrderPayPage />} />
        <Route path="/ledger" element={<LedgerPage />} />
        <Route path="/security" element={<Navigate to="/security/events" replace />} />
        <Route path="/security/events" element={<NodeSecurityEventsPage />} />
        <Route path="/account/security" element={<AccountSecurityPage />} />
        <Route path="/audit-logs" element={<ResourcePage endpoint="/v1/audit-logs" />} />
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
  if (!account || (account.role !== 'user' && account.role !== 'admin')) {
    return <Navigate to="/login" replace />
  }
  return <AppLayout />
}
