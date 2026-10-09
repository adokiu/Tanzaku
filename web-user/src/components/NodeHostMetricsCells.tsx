import { useTranslation } from 'react-i18next'
import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { formatBytes, formatNetSpeed, progressColor } from '@/utils/formatMetrics'

export type HostMetrics = {
  cpu_usage_percent: number
  cpu_cores: number
  memory_used_bytes: number
  memory_total_bytes: number
  net_up_bps: number
  net_down_bps: number
  connections_tcp?: number
  connections_udp?: number
}

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

function EmptyMetric() {
  const { t } = useTranslation()
  return <span className="text-muted-foreground">{t('metrics.empty')}</span>
}

export function NodeCpuCell({ metrics }: { metrics: HostMetrics | null | undefined }) {
  const { t } = useTranslation()
  if (!metrics) return <EmptyMetric />
  const used = metrics.cpu_usage_percent
  const total = metrics.cpu_cores
  return (
    <div className="metric-cell">
      <div className="metric-cell__header">
        <span className="metric-cell__line">
          {t('node.cpuSummary', { percent: used.toFixed(1), cores: total })}
        </span>
      </div>
      <MetricBar percent={used} />
    </div>
  )
}

export function NodeMemoryCell({ metrics }: { metrics: HostMetrics | null | undefined }) {
  if (!metrics || !metrics.memory_total_bytes) return <EmptyMetric />
  const percent = (metrics.memory_used_bytes / metrics.memory_total_bytes) * 100
  const memDetail = `${formatBytes(metrics.memory_used_bytes)} / ${formatBytes(metrics.memory_total_bytes)}`
  return (
    <div className="metric-cell metric-cell--memory">
      <div className="metric-cell__header metric-cell__header--memory">
        <span className="metric-cell__percent">{percent.toFixed(1)}%</span>
        <CellTooltip tip={memDetail} className="metric-cell__mem-detail">
          {memDetail}
        </CellTooltip>
      </div>
      <MetricBar percent={percent} />
    </div>
  )
}

export function NodeNetCell({ metrics }: { metrics: HostMetrics | null | undefined }) {
  const { t } = useTranslation()
  if (!metrics) return <EmptyMetric />
  const up = formatNetSpeed(metrics.net_up_bps)
  const down = formatNetSpeed(metrics.net_down_bps)
  return (
    <div className="net-cell">
      <span className="net-cell__line">
        <span className="net-cell__up">{t('node.netUpload', { speed: up })}</span>
        <span className="net-cell__sep"> </span>
        <span className="net-cell__down">{t('node.netDownload', { speed: down })}</span>
      </span>
    </div>
  )
}

export function NodeConnectionsCell({ metrics }: { metrics: HostMetrics | null | undefined }) {
  if (!metrics) return <EmptyMetric />
  const total = (metrics.connections_tcp ?? 0) + (metrics.connections_udp ?? 0)
  return <span className="data-table-cell">{total}</span>
}
