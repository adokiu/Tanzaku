import { daysUntilDate, formatDateYmd } from '@/utils/formatDateTime'

export type CertExpiryTone = 'ok' | 'warn' | 'urgent' | 'expired' | 'unknown'

export function certExpiryTone(notAfter: unknown): CertExpiryTone {
  const days = daysUntilDate(notAfter)
  if (days === null) return 'unknown'
  if (days < 0) return 'expired'
  if (days <= 7) return 'urgent'
  if (days <= 30) return 'warn'
  return 'ok'
}

const TONE_CLASS: Record<CertExpiryTone, string> = {
  ok: 'data-table-tag--cert-ok',
  warn: 'data-table-tag--cert-warn',
  urgent: 'data-table-tag--cert-urgent',
  expired: 'data-table-tag--cert-expired',
  unknown: 'data-table-tag--neutral',
}

export function CertExpiryTag({
  notAfter,
  className = '',
}: {
  notAfter: unknown
  className?: string
}) {
  const text = formatDateYmd(notAfter)
  if (!text) return <span className="data-table-tag data-table-tag--neutral">—</span>
  const tone = certExpiryTone(notAfter)
  return (
    <span className={`data-table-tag entity-list-cell__status-tag ${TONE_CLASS[tone]} ${className}`.trim()}>
      {text}
    </span>
  )
}
