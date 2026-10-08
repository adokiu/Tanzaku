import { useTranslation } from 'react-i18next'
import { formatBytes, progressColor } from '@/utils/formatMetrics'

function MetricBar({ percent }: { percent: number }) {
  const color = progressColor(percent)
  return (
    <div className="metric-progress">
      <div
        className={`metric-progress__fill metric-progress__fill--${color}`}
        style={{ width: `${Math.min(Math.max(percent, 0), 100)}%` }}
      />
    </div>
  )
}

export function UserAccountStatusCell({ status }: { status: string }) {
  const { t } = useTranslation()
  const active = status === 'active'
  return (
    <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--status data-table-cell">
      <span
        className={`data-table-tag entity-list-cell__status-tag ${active ? 'data-table-tag--online' : 'data-table-tag--disabled'}`}
      >
        {active ? t('common.enabled') : t('common.disabled')}
      </span>
    </span>
  )
}

export function UserRoleCell({ role }: { role: string }) {
  const { t } = useTranslation()
  const admin = role === 'admin'
  return (
    <span className="entity-list-cell data-table-cell">
      <span
        className={`data-table-tag entity-list-cell__status-tag ${admin ? 'data-table-tag--role-admin' : 'data-table-tag--role-user'}`}
      >
        {admin ? t('users.roleAdmin') : t('users.roleUser')}
      </span>
    </span>
  )
}

export function UserSubscriptionCell({
  planName,
  subscriptionStatus,
}: {
  planName: string | null | undefined
  subscriptionStatus: string | null | undefined
}) {
  const { t } = useTranslation()
  const name = planName?.trim()
  if (!name) {
    return <span className="data-table-cell text-muted-foreground">{t('users.noSubscription')}</span>
  }
  const title =
    subscriptionStatus && subscriptionStatus !== 'active'
      ? `${name} (${subscriptionStatus})`
      : name
  return (
    <span className="entity-list-cell entity-list-cell--name data-table-cell" title={title}>
      {name}
    </span>
  )
}

/** 已用流量：左大字用量、右小字百分比 + 进度条（同内存列结构） */
export function UserTrafficUsedCell({
  usedBytes,
  quotaBytes,
}: {
  usedBytes: number
  quotaBytes: number | null | undefined
}) {
  const used = Number.isFinite(usedBytes) ? usedBytes : 0
  const quota = quotaBytes != null && quotaBytes > 0 ? quotaBytes : null
  const percent = quota != null ? (used / quota) * 100 : 0
  const detailTitle =
    quota != null ? `${formatBytes(used)} / ${formatBytes(quota)}` : formatBytes(used)

  return (
    <div className="metric-cell metric-cell--memory metric-cell--user-traffic">
      <div className="metric-cell__header metric-cell__header--memory">
        <span className="metric-cell__user-traffic-bytes" title={detailTitle}>
          {formatBytes(used)}
        </span>
        <span className="metric-cell__user-traffic-pct">{percent.toFixed(1)}%</span>
      </div>
      <MetricBar percent={percent} />
    </div>
  )
}

/** 总流量（周期配额） */
export function UserTrafficTotalCell({ quotaBytes }: { quotaBytes: number | null | undefined }) {
  const { t } = useTranslation()
  const quota = quotaBytes != null && quotaBytes > 0 ? quotaBytes : null
  if (quota == null) {
    return <span className="data-table-cell text-muted-foreground">{t('users.trafficUnlimited')}</span>
  }
  return <span className="data-table-cell">{formatBytes(quota)}</span>
}
