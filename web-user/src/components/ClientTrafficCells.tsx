import { useTranslation } from 'react-i18next'
import { formatBytes, formatNetSpeed } from '@/utils/formatMetrics'

export type ClientTrafficMetrics = {
  bytes_in: number
  bytes_out: number
  in_bps: number
  out_bps: number
}

function EmptyMetric() {
  const { t } = useTranslation()
  return <span className="text-muted-foreground">{t('metrics.empty')}</span>
}

export function ClientTrafficInCell({ metrics }: { metrics: ClientTrafficMetrics | null | undefined }) {
  if (!metrics) return <EmptyMetric />
  return <span className="data-table-cell">{formatBytes(metrics.bytes_in)}</span>
}

export function ClientTrafficOutCell({ metrics }: { metrics: ClientTrafficMetrics | null | undefined }) {
  if (!metrics) return <EmptyMetric />
  return <span className="data-table-cell">{formatBytes(metrics.bytes_out)}</span>
}

export function ClientTrafficSpeedCell({ metrics }: { metrics: ClientTrafficMetrics | null | undefined }) {
  const { t } = useTranslation()
  if (!metrics) return <EmptyMetric />
  const down = formatNetSpeed(metrics.in_bps)
  const up = formatNetSpeed(metrics.out_bps)
  return (
    <div className="net-cell data-table-cell">
      <span className="net-cell__line">
        <span className="net-cell__down">{t('node.netDownload', { speed: down })}</span>
        <span className="net-cell__sep"> </span>
        <span className="net-cell__up">{t('node.netUpload', { speed: up })}</span>
      </span>
    </div>
  )
}
