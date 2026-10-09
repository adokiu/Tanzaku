import { useCallback, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { PageListHeader } from '@/components/PageListHeader'
import { formatDateTime } from '@/utils/formatDateTime'

type LedgerRow = {
  id: string
  kind: string
  amount_cents: number
  title: string
  created_at: string
}

export default function LedgerPage() {
  const { t } = useTranslation()
  const columns = useMemo<Column<LedgerRow>[]>(
    () => [
      { key: 'kind', title: t('ledger.kind') },
      { key: 'title', title: t('ledger.title') },
      {
        key: 'amount_cents',
        title: t('ledger.amount'),
        render: (row) => `¥${(row.amount_cents / 100).toFixed(2)}`,
      },
      {
        key: 'created_at',
        title: t('ledger.createdAt'),
        render: (row) => formatDateTime(row.created_at),
      },
    ],
    [t],
  )

  const fetchPage = useCallback(async (params: { page: number; page_size: number }) => {
    const response = await apiClient.get('/v1/me/ledger', { params })
    const data = response.data as {
      items: LedgerRow[]
      total: number
      page: number
      page_size: number
    }
    return {
      items: data.items ?? [],
      total: data.total ?? 0,
      page: data.page ?? params.page,
      page_size: data.page_size ?? params.page_size,
    }
  }, [])

  return (
    <section className="page-container">
      <PageListHeader title={t('pages.ledger')} />
      <EntityList
        columns={columns}
        queryKey={['user-ledger']}
        fetchPage={fetchPage}
        rowKey={(row) => String(row.id)}
        emptyText={t('ledger.empty')}
      />
    </section>
  )
}
