import { useEffect, useMemo, useRef, useState } from 'react'
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Receipt,
  Globe,
  LayoutDashboard,
  LogOut,
  Moon,
  Package,
  Palette,
  Route,
  ScrollText,
  Server,
  Settings,
  Shield,
  ShieldCheck,
  Sun,
  Users,
  Workflow,
} from 'lucide-react'
import apiClient from '@/api/client'
import { useAuthStore } from '@/stores/auth'
import { useThemeStore } from '@/stores/theme'
import './AppLayout.css'

type IconType = typeof LayoutDashboard
type NavItem = { path: string; labelKey: string; icon: IconType }
type NavGroup = { id: string; labelKey: string; icon: IconType; items: NavItem[] }

function groupForPath(groups: NavGroup[], pathname: string): string | undefined {
  return groups.find((group) => group.items.some((item) => item.path === pathname))?.id
}

function NavSubLink({ item, label, onNavigate }: { item: NavItem; label: string; onNavigate?: () => void }) {
  const ItemIcon = item.icon
  return (
    <NavLink
      to={item.path}
      className={({ isActive }) => `nav-sub-item menu-item ${isActive ? 'active' : ''}`}
      onClick={onNavigate}
    >
      <ItemIcon size={20} className="menu-item-icon" />
      <span className="menu-item-label">{label}</span>
    </NavLink>
  )
}

export default function AppLayout() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const contentRef = useRef<HTMLDivElement>(null)
  const groups = useMemo<NavGroup[]>(() => [
    {
      id: 'dashboard',
      labelKey: 'nav.groupDashboard',
      icon: LayoutDashboard,
      items: [{ path: '/dashboard', labelKey: 'nav.overview', icon: LayoutDashboard }],
    },
    {
      id: 'users',
      labelKey: 'nav.groupUserManagement',
      icon: Users,
      items: [{ path: '/users', labelKey: 'nav.users', icon: Users }],
    },
    {
      id: 'nodes',
      labelKey: 'nav.groupNodeManagement',
      icon: Server,
      items: [
        { path: '/nodes', labelKey: 'nav.nodes', icon: Server },
        { path: '/clients', labelKey: 'nav.clients', icon: Workflow },
        { path: '/tunnels', labelKey: 'nav.tunnels', icon: Route },
        { path: '/certificates', labelKey: 'nav.certificates', icon: ShieldCheck },
      ],
    },
    {
      id: 'security',
      labelKey: 'nav.groupSecurity',
      icon: Shield,
      items: [
        { path: '/security', labelKey: 'nav.securityPolicy', icon: Shield },
        { path: '/security/events', labelKey: 'nav.securityEvents', icon: ScrollText },
        { path: '/security/domains', labelKey: 'nav.domainWhitelist', icon: Globe },
      ],
    },
    {
      id: 'subscriptions',
      labelKey: 'nav.groupSubscriptionManagement',
      icon: Workflow,
      items: [
        { path: '/plans', labelKey: 'nav.plans', icon: Package },
        { path: '/orders', labelKey: 'nav.orders', icon: Receipt },
      ],
    },
    {
      id: 'system',
      labelKey: 'nav.groupSystemManagement',
      icon: Settings,
      items: [
        { path: '/settings', labelKey: 'nav.settings', icon: Settings },
        { path: '/themes', labelKey: 'nav.themes', icon: Palette },
        { path: '/audit-logs', labelKey: 'nav.auditLogs', icon: ScrollText },
      ],
    },
  ], [])

  const account = useAuthStore((state) => state.account)
  const setAccount = useAuthStore((state) => state.setAccount)
  const theme = useThemeStore((state) => state.theme)
  const setTheme = useThemeStore((state) => state.setTheme)
  const [collapsed, setCollapsed] = useState(false)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [flyoutGroupId, setFlyoutGroupId] = useState<string | null>(null)
  const flyoutRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const activeId = groupForPath(groups, location.pathname)
    if (activeId) {
      setExpanded((current) => ({ ...current, [activeId]: true }))
      setFlyoutGroupId(null)
    }
  }, [location.pathname, groups])

  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0, left: 0 })
  }, [location.pathname])

  useEffect(() => {
    if (!flyoutGroupId) return
    function onPointerDown(event: MouseEvent) {
      const target = event.target as Node
      if (flyoutRef.current?.contains(target)) return
      if ((target as Element).closest?.('.nav-group-trigger')) return
      setFlyoutGroupId(null)
    }
    document.addEventListener('mousedown', onPointerDown)
    return () => document.removeEventListener('mousedown', onPointerDown)
  }, [flyoutGroupId])

  async function logout() {
    try {
      await apiClient.post('/auth/logout')
    } catch {
      setAccount(null)
    } finally {
      setAccount(null)
      navigate('/login', { replace: true })
    }
  }

  function toggleTheme() {
    setTheme(theme === 'light' ? 'dark' : theme === 'dark' ? 'system' : 'light')
  }

  function toggleGroup(groupId: string) {
    if (collapsed) {
      setFlyoutGroupId((current) => (current === groupId ? null : groupId))
      return
    }
    setExpanded((current) => ({ ...current, [groupId]: !current[groupId] }))
  }

  const flyoutGroup = flyoutGroupId ? groups.find((group) => group.id === flyoutGroupId) : undefined

  return (
    <div className="app-shell">
      <aside className={`sidebar-container ${collapsed ? 'collapsed' : 'expanded'}`}>
        <div className={`sidebar-header ${collapsed ? 'collapsed-header' : ''}`}>
          {!collapsed ? <div className="sidebar-logo-text">Tanzaku</div> : <span className="sidebar-logo-text">T</span>}
        </div>
        <nav className="sidebar-nav" aria-label={t('nav.brandAdmin')}>
          {groups.map((group) => {
            const Icon = group.icon
            const isOpen = collapsed ? flyoutGroupId === group.id : Boolean(expanded[group.id])
            return (
              <div key={group.id} className={`nav-group ${isOpen ? 'is-open' : ''}`}>
                <button
                  type="button"
                  className="nav-group-trigger menu-item"
                  aria-expanded={isOpen}
                  title={t(group.labelKey)}
                  onClick={() => toggleGroup(group.id)}
                >
                  <Icon size={20} className="menu-item-icon" />
                  {!collapsed && (
                    <>
                      <span className="menu-item-label">{t(group.labelKey)}</span>
                      <ChevronDown size={18} className={`nav-group-chevron ${isOpen ? 'is-open' : ''}`} aria-hidden />
                    </>
                  )}
                </button>
                {!collapsed && (
                  <div className={`nav-sub-wrap ${isOpen ? 'is-open' : ''}`}>
                    <div className="nav-sub-inner">
                      <ul className="nav-sub-list">
                        {group.items.map((item) => (
                          <li key={item.path}>
                            <NavSubLink item={item} label={t(item.labelKey)} />
                          </li>
                        ))}
                      </ul>
                    </div>
                  </div>
                )}
              </div>
            )
          })}
          <div className="sidebar-spacer" />
          <ul className="menu-list menu-list-footer">
            <li>
              <button type="button" className="menu-item" onClick={toggleTheme} title={t('nav.themeToggle')}>
                {theme === 'dark' ? <Moon size={18} className="menu-item-icon" /> : <Sun size={18} className="menu-item-icon" />}
                {!collapsed && <span className="menu-item-label">{t('nav.themeToggle')}</span>}
              </button>
            </li>
            <li>
              <button type="button" className="menu-item" onClick={logout} title={t('nav.logout')}>
                <LogOut size={18} className="menu-item-icon" />
                {!collapsed && <span className="menu-item-label">{t('nav.logout')}</span>}
              </button>
            </li>
          </ul>
        </nav>
        {collapsed && flyoutGroup && (
          <div ref={flyoutRef} className="sidebar-flyout" role="menu">
            <p className="sidebar-flyout-title">{t(flyoutGroup.labelKey)}</p>
            <ul className="nav-sub-list">
              {flyoutGroup.items.map((item) => (
                <li key={item.path}>
                  <NavSubLink item={item} label={t(item.labelKey)} onNavigate={() => setFlyoutGroupId(null)} />
                </li>
              ))}
            </ul>
          </div>
        )}
        <button
          type="button"
          className="sidebar-collapse-btn"
          onClick={() => {
            setCollapsed((value) => !value)
            setFlyoutGroupId(null)
          }}
          aria-label={collapsed ? t('nav.expandMenu') : t('nav.collapseMenu')}
        >
          {collapsed ? <ChevronRight size={20} /> : <ChevronLeft size={20} />}
        </button>
      </aside>
      <main className="app-main">
        <header className="app-topbar"><span>{account?.email}</span></header>
        <div className="app-content" ref={contentRef}><Outlet /></div>
      </main>
    </div>
  )
}
