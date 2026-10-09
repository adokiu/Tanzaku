import { useCallback, useMemo } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { getPage } from '@/api/page'
import { Button } from '@/components/Button'
import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { PageListHeader } from '@/components/PageListHeader'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime, formatDurationSecs } from '@/utils/formatDateTime'
import { translateGuardRule } from './security/policyShared'
import './security/security.css'

type GuardEvent = {
  id: number
  node_id: string
  node_name: string
  rule: string
  tunnel_id: string | null
  intensity: number
  duration_secs: number
  first_seen_at: string
  last_seen_at: string
}

export default function NodeSecurityEventsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<GuardEvent>('/v1/me/security/events', params),
    [],
  )

  const eventColumns: Column<GuardEvent>[] = useMemo(
    () => [
      {
        key: 'last_seen_at',
        title: t('security.lastSeen'),
        width: 168,
        render: (row) => <span className="data-table-cell">{formatDateTime(row.last_seen_at)}</span>,
      },
      { key: 'node_name', title: translateField(t, 'node_name'), render: (row) => <span className="data-table-cell">{row.node_name}</span> },
      {
        key: 'rule',
        title: t('security.rule'),
        render: (row) => (
          <span className="data-table-cell">
            <CellTooltip tip={row.rule} className="data-table-tag data-table-tag--neutral">
              {translateGuardRule(t, row.rule)}
            </CellTooltip>
          </span>
        ),
      },
      {
        key: 'tunnel_id',
        title: t('security.tunnelId'),
        render: (row) => <span className="data-table-cell">{row.tunnel_id ?? '—'}</span>,
      },
      {
        key: 'intensity',
        title: t('security.intensity'),
        width: 88,
        render: (row) => <span className="data-table-cell">{row.intensity}</span>,
      },
      {
        key: 'duration_secs',
        title: t('security.duration'),
        width: 96,
        render: (row) => <span className="data-table-cell">{formatDurationSecs(row.duration_secs)}</span>,
      },
    ],
    [t],
  )

  return (
    <section className="page-container">
      <PageListHeader
        actions={(
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void queryClient.invalidateQueries({ queryKey: ['user-security-events'] })}
          >
            <RefreshCw size={16} />
            {t('common.refresh')}
          </Button>
        )}
      />

      <EntityList
        columns={eventColumns}
        queryKey={['user-security-events']}
        fetchPage={fetchPage}
        rowKey={(row) => String(row.id)}
        refetchInterval={3000}
        emptyText={t('security.eventsEmpty')}
        tableClassName=""
      />
    </section>
  )
}
