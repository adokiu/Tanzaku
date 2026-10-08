import { useTranslation } from 'react-i18next'
import type { ClientTrafficMetrics } from '@/components/ClientTrafficCells'
import { formatBytes, formatNetSpeed } from '@/utils/formatMetrics'

const ZERO_BYTES_LABEL = '0.00 B'
const ZERO_SPEED_LABEL = '0.00 B/s'

function formatTunnelTotalBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return ZERO_BYTES_LABEL
  return formatBytes(bytes)
}

/** API `in_bps` / `out_bps` 为 bit/s，展示用 byte/s */
function formatTunnelSpeed(bitsPerSec: number): string {
  if (!Number.isFinite(bitsPerSec) || bitsPerSec <= 0) return ZERO_SPEED_LABEL
  return formatNetSpeed(bitsPerSec / 8)
}

function TagCell({ className, children }: { className: string; children: string }) {
  return (
    <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--status data-table-cell">
      <span className={`data-table-tag entity-list-cell__status-tag ${className}`}>
        {children || '—'}
      </span>
    </span>
  )
}

export function TunnelCarrierTag({ carrier }: { carrier: string }) {
  const normalized = (carrier ?? '').trim().toLowerCase() || '—'
  const className =
    normalized === 'quic'
      ? 'data-table-tag--carrier-quic'
      : normalized === 'tcp'
        ? 'data-table-tag--carrier-tcp'
        : 'data-table-tag--neutral'
  return <TagCell className={className}>{normalized === '—' ? '—' : normalized.toUpperCase()}</TagCell>
}

export function TunnelProtocolTag({ protocol }: { protocol: string }) {
  const normalized = (protocol ?? '').trim().toLowerCase() || '—'
  const className =
    normalized === 'udp'
      ? 'data-table-tag--protocol-udp'
      : normalized === 'http'
        ? 'data-table-tag--protocol-http'
        : normalized === 'tcp'
          ? 'data-table-tag--protocol-tcp'
          : 'data-table-tag--neutral'
  return <TagCell className={className}>{normalized === '—' ? '—' : normalized.toUpperCase()}</TagCell>
}

const TUNNEL_STATUS_TAG: Record<string, string> = {
  active: 'data-table-tag--online',
  provisioning: 'data-table-tag--tunnel-pending',
  pending_review: 'data-table-tag--tunnel-pending',
  error: 'data-table-tag--offline',
  suspended: 'data-table-tag--muted',
  disabled: 'data-table-tag--muted',
  deleted: 'data-table-tag--muted',
}

function normalizeTunnelStatus(status: string): string {
  const normalized = (status ?? '').trim().toLowerCase()
  if (normalized === 'ready' || normalized === 'client_offline' || normalized === 'node_offline') {
    return 'active'
  }
  if (normalized === 'failed') {
    return 'error'
  }
  return normalized || 'active'
}

export function TunnelStatusCell({ status, enabled }: { status: string; enabled: boolean }) {
  const { t } = useTranslation()
  const normalized = normalizeTunnelStatus(status)
  let tagClass = TUNNEL_STATUS_TAG[normalized] ?? 'data-table-tag--neutral'
  let labelKey = `tunnels.status.${normalized}`

  if (!enabled && normalized !== 'suspended' && normalized !== 'pending_review') {
    tagClass = 'data-table-tag--muted'
    labelKey = 'tunnels.status.disabled'
  }

  const label = t(labelKey, { defaultValue: normalized || '—' })
  return <TagCell className={tagClass}>{label}</TagCell>
}

export function TunnelTargetCell({
  targetHost,
  targetPort,
  targetUrl,
}: {
  targetHost?: string | null
  targetPort?: number | null
  targetUrl?: string | null
}) {
  if (targetUrl?.trim()) {
    return <span className="data-table-cell data-table-cell--truncate">{targetUrl.trim()}</span>
  }
  const host = targetHost?.trim()
  if (!host) return <span className="text-muted-foreground">—</span>
  const port = targetPort != null && targetPort > 0 ? `:${targetPort}` : ''
  return (
    <span className="data-table-cell">
      {host}
      {port}
    </span>
  )
}

export function TunnelTrafficTotalCell({
  metrics,
}: {
  metrics: ClientTrafficMetrics | null | undefined
}) {
  const total = (metrics?.bytes_in ?? 0) + (metrics?.bytes_out ?? 0)
  return <span className="data-table-cell tabular-nums">{formatTunnelTotalBytes(total)}</span>
}

export function TunnelTrafficSpeedCell({
  metrics,
}: {
  metrics: ClientTrafficMetrics | null | undefined
}) {
  const { t } = useTranslation()
  const down = formatTunnelSpeed(metrics?.in_bps ?? 0)
  const up = formatTunnelSpeed(metrics?.out_bps ?? 0)
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