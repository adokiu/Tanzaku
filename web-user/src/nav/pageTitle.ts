/** 用户端路由 → 页面标题 i18n key（与侧栏 nav 文案一致） */
const PAGE_TITLE_KEYS: Record<string, string> = {
  '/dashboard': 'nav.overview',
  '/plans': 'nav.plans',
  '/orders': 'nav.orders',
  '/ledger': 'nav.ledger',
  '/clients': 'nav.clients',
  '/tunnels': 'nav.tunnels',
  '/certificates': 'nav.certificates',
  '/security/events': 'nav.securityEvents',
  '/account/security': 'nav.accountSecurity',
  '/audit-logs': 'nav.auditLogs',
}

export function pageTitleKeyForPath(pathname: string): string {
  if (PAGE_TITLE_KEYS[pathname]) return PAGE_TITLE_KEYS[pathname]
  const hit = Object.keys(PAGE_TITLE_KEYS)
    .filter((path) => path !== '/' && pathname.startsWith(`${path}/`))
    .sort((a, b) => b.length - a.length)[0]
  return hit ? PAGE_TITLE_KEYS[hit] : 'nav.brandUser'
}
