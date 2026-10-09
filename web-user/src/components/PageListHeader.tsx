import type { ReactNode } from 'react'
import { useLocation } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { pageTitleKeyForPath } from '@/nav/pageTitle'

export function PageListHeader({
  title,
  titleKey,
  actions,
}: {
  /** 直接传入标题文案；优先于 titleKey / 路由推断 */
  title?: string
  /** i18n key；未传时按当前路由推断 */
  titleKey?: string
  actions?: ReactNode
}) {
  const { t } = useTranslation()
  const location = useLocation()
  const resolved =
    title?.trim()
    || t(titleKey || pageTitleKeyForPath(location.pathname))

  return (
    <div className="page-header">
      <h1 className="page-header__title">{resolved}</h1>
      {actions ? <div className="page-header__actions">{actions}</div> : null}
    </div>
  )
}
