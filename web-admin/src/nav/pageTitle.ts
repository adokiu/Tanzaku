/** 后台路由 → 页面标题 i18n key（与侧栏 nav 文案一致） */
const PAGE_TITLE_KEYS: Record<string, string> = {
  '/dashboard': 'nav.overview',
  '/nodes': 'nav.nodes',
  '/users': 'nav.users',
  '/plans': 'nav.plans',
  '/orders': 'nav.orders',
  '/payments': 'nav.payments',
  '/clients': 'nav.clients',
  '/tunnels': 'nav.tunnels',
  '/certificates': 'nav.certificates',
  '/security': 'nav.securityPolicy',
  '/security/events': 'nav.securityEvents',
  '/security/domains': 'nav.domainWhitelist',
  '/settings': 'nav.settings',
  '/themes': 'nav.themes',
  '/audit-logs': 'nav.auditLogs',
}

export function pageTitleKeyForPath(pathname: string): string {
  if (PAGE_TITLE_KEYS[pathname]) return PAGE_TITLE_KEYS[pathname]
  const hit = Object.keys(PAGE_TITLE_KEYS)
    .filter((path) => path !== '/' && pathname.startsWith(`${path}/`))
    .sort((a, b) => b.length - a.length)[0]
  return hit ? PAGE_TITLE_KEYS[hit] : 'nav.brandAdmin'
}
