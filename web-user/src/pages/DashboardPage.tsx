import type { ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { formatBytes } from '@/utils/formatMetrics'
import { formatDateYmd } from '@/utils/formatDateTime'

type Overview = {
  subscription_plan_name: string | null
  subscription_status: string | null
  subscription_expires_at: string | null
  traffic_used_bytes: number
  traffic_quota_bytes: number | null
  traffic_exhausted: boolean
  client_count: number
  tunnel_count: number
  balance_cents: number
}

export default function DashboardPage() {
  const { t } = useTranslation()
  const query = useQuery({
    queryKey: ['me-overview'],
    queryFn: async () => (await apiClient.get('/v1/me/overview')).data as Overview,
  })
  const data = query.data

  return (
    <section className="page-container">
      <PageListHeader title={t('pages.dashboard')} />
      {query.isError ? (
        <div className="page-card p-6 text-sm text-destructive">{translateApiError(t, query.error)}</div>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <StatCard label={t('dashboard.plan')} value={data?.subscription_plan_name ?? '—'} hint={data?.subscription_status ?? ''} />
          <StatCard label={t('dashboard.clients')} value={String(data?.client_count ?? '—')} />
          <StatCard label={t('dashboard.tunnels')} value={String(data?.tunnel_count ?? '—')} />
          <StatCard
            label={t('dashboard.balance')}
            value={data ? `¥${(data.balance_cents / 100).toFixed(2)}` : '—'}
          />
          <StatCard
            label={t('dashboard.traffic')}
            value={data ? formatBytes(data.traffic_used_bytes) : '—'}
            hint={
              data?.traffic_quota_bytes != null
                ? `${formatBytes(data.traffic_quota_bytes)}${data.traffic_exhausted ? ' · exhausted' : ''}`
                : undefined
            }
          />
          <StatCard
            label={t('dashboard.expires')}
            value={data?.subscription_expires_at ? formatDateYmd(data.subscription_expires_at) : '—'}
          />
        </div>
      )}
    </section>
  )
}

function StatCard({ label, value, hint }: { label: string; value: string; hint?: ReactNode }) {
  return (
    <article className="page-card p-5">
      <div className="text-xs text-muted-foreground mb-1">{label}</div>
      <div className="text-lg font-semibold">{value}</div>
      {hint ? <div className="text-xs text-muted-foreground mt-2">{hint}</div> : null}
    </article>
  )
}
