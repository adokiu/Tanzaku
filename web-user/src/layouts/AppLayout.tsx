import { useState } from 'react'
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import { ChevronLeft, ChevronRight, LayoutDashboard, LogOut, Moon, Network, Package, Server, Shield, Sun, Users, Workflow } from 'lucide-react'
import apiClient from '@/api/client'
import { useAuthStore } from '@/stores/auth'
import { useThemeStore } from '@/stores/theme'
import './AppLayout.css'

type Audience = 'admin' | 'user'
type Item = { path: string; label: string; icon: typeof LayoutDashboard }
type Group = { id: string; label: string; icon: typeof LayoutDashboard; items: Item[] }

const adminGroups: Group[] = [
  { id: 'overview', label: '概览', icon: LayoutDashboard, items: [{ path: '/dashboard', label: '概览', icon: LayoutDashboard }] },
  { id: 'nodes', label: '节点管理', icon: Server, items: [
    { path: '/nodes', label: 'Server 节点', icon: Server },
    { path: '/node-groups', label: '节点组', icon: Network },
  ] },
  { id: 'users', label: '用户与套餐', icon: Users, items: [
    { path: '/users', label: '用户管理', icon: Users },
    { path: '/plans', label: '套餐管理', icon: Package },
  ] },
  { id: 'tunnels', label: '转发管理', icon: Network, items: [
    { path: '/clients', label: 'Client 管理', icon: Workflow },
    { path: '/tunnels', label: '隧道管理', icon: Network },
    { path: '/certificates', label: '证书管理', icon: Shield },
  ] },
  { id: 'system', label: '系统管理', icon: Shield, items: [
    { path: '/settings', label: '系统设置', icon: LayoutDashboard },
    { path: '/security', label: '节点防护', icon: Shield },
    { path: '/audit-logs', label: '审计日志', icon: LayoutDashboard },
  ] },
]

const userGroups: Group[] = [
  { id: 'overview', label: '概览', icon: LayoutDashboard, items: [{ path: '/overview', label: '概览', icon: LayoutDashboard }] },
  { id: 'plans', label: '套餐', icon: Package, items: [{ path: '/plans', label: '购买套餐', icon: Package }] },
  { id: 'tunnels', label: '我的转发', icon: Network, items: [
    { path: '/clients', label: '我的 Client', icon: Workflow },
    { path: '/tunnels', label: '我的隧道', icon: Network },
    { path: '/tunnels/new', label: '创建隧道', icon: Network },
  ] },
]

export default function AppLayout({ audience }: { audience: Audience }) {
  const navigate = useNavigate()
  const location = useLocation()
  const groups = audience === 'admin' ? adminGroups : userGroups
  const activeGroup = groups.find((group) => group.items.some((item) => item.path === location.pathname))?.id
  const account = useAuthStore((state) => state.account)
  const setAccount = useAuthStore((state) => state.setAccount)
  const theme = useThemeStore((state) => state.theme)
  const setTheme = useThemeStore((state) => state.setTheme)
  const [collapsed, setCollapsed] = useState(false)

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

  return (
    <div className="app-shell">
      <aside className={`sidebar-container ${collapsed ? 'collapsed' : 'expanded'}`}>
        <div className={`sidebar-header ${collapsed ? 'collapsed-header' : ''}`}>
          {!collapsed ? <div className="sidebar-logo-text">Tanzaku</div> : <span className="sidebar-logo-text">T</span>}
          {!collapsed && <div className="sidebar-search">{audience === 'admin' ? '管理控制台' : '用户中心'}</div>}
        </div>
        <div className="sidebar-menus">
          <nav className="sidebar-primary">
            <ul className="menu-list">
              {groups.map((group) => {
                const Icon = group.icon
                return <li key={group.id}><button type="button" className={`menu-item ${activeGroup === group.id ? 'active' : ''}`} title={group.label} onClick={() => navigate(group.items[0].path)}><Icon size={18} className="menu-item-icon" />{!collapsed && <span className="menu-item-label">{group.label}</span>}</button></li>
              })}
            </ul>
            <div className="sidebar-spacer" />
            <ul className="menu-list">
              <li><button type="button" className="menu-item" onClick={toggleTheme} title="切换主题">{theme === 'dark' ? <Moon size={18} className="menu-item-icon" /> : <Sun size={18} className="menu-item-icon" />}{!collapsed && <span className="menu-item-label">主题</span>}</button></li>
              <li><button type="button" className="menu-item" onClick={logout} title="退出登录"><LogOut size={18} className="menu-item-icon" />{!collapsed && <span className="menu-item-label">退出登录</span>}</button></li>
            </ul>
          </nav>
          {!collapsed && <nav className="sidebar-secondary"><ul className="secondary-menu-list">{groups.flatMap((group) => group.items).map((item) => {
            const Icon = item.icon
            return <li key={item.path}><NavLink to={item.path} className={({ isActive }) => `secondary-menu-item ${isActive ? 'active' : ''}`}><Icon size={16} className="secondary-menu-item-icon" /><span>{item.label}</span></NavLink></li>
          })}</ul></nav>}
        </div>
        <button type="button" className="sidebar-collapse-btn" onClick={() => setCollapsed((value) => !value)} aria-label={collapsed ? '展开菜单' : '收起菜单'}>{collapsed ? <ChevronRight size={20} /> : <ChevronLeft size={20} />}</button>
      </aside>
      <main className="app-main">
        <header className="app-topbar"><span>{account?.email}</span></header>
        <div className="app-content"><Outlet /></div>
      </main>
    </div>
  )
}
