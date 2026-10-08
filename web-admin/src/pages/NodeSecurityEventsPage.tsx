import { useCallback, useMemo } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { getPage } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { PageListHeader } from '@/components/PageListHeader'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime } from '@/utils/formatDateTime'
import { translateGuardRule } from './security/policyShared'
import './security/security.css'

type GuardEvent = {
  id: number
  node_id: string
  node_name: string
  rule: string
  peer: string | null
  tunnel_id: string | null
  detail: string
  hit_count: number
  first_seen_at: string
  last_seen_at: string
}

export default function NodeSecurityEventsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<GuardEvent>('/v1/admin/security/events', params),
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
            <span className="data-table-tag data-table-tag--neutral" title={row.rule}>
              {translateGuardRule(t, row.rule)}
            </span>
          </span>
        ),
      },
      { key: 'peer', title: t('security.peer'), render: (row) => <span className="data-table-cell">{row.peer ?? '—'}</span> },
      {
        key: 'tunnel_id',
        title: t('security.tunnelId'),
        render: (row) => <span className="data-table-cell">{row.tunnel_id ?? '—'}</span>,
      },
      { key: 'hit_count', title: t('security.hitCount'), width: 72, render: (row) => <span className="data-table-cell">{row.hit_count}</span> },
      {
        key: 'detail',
        title: t('security.detail'),
        render: (row) => <span className="data-table-cell data-table-cell--truncate">{row.detail || '—'}</span>,
      },
    ],
    [t],
  )

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.securityEvents')}
        actions={(
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-security-events'] })}
          >
            <RefreshCw size={16} />
            {t('common.refresh')}
          </Button>
        )}
      />

      <EntityList
        columns={eventColumns}
        queryKey={['admin-security-events']}
        fetchPage={fetchPage}
        rowKey={(row) => String(row.id)}
        refetchInterval={3000}
        emptyText={t('security.eventsEmpty')}
        tableClassName=""
      />
    </section>
  )
}
