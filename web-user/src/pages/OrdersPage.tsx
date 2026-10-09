import { useMemo } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { getPage } from '@/api/page'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { PageListHeader } from '@/components/PageListHeader'
import { formatDateTime } from '@/utils/formatDateTime'
import { formatBalanceCents } from '@/utils/formatMoney'

type OrderRow = {
  id: string
  order_no: string
  plan_name?: string | null
  period: string
  amount_cents: number
  status: string
  created_at: string
}

export default function OrdersPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const columns = useMemo<Column<OrderRow>[]>(
    () => [
      {
        key: 'order_no',
        title: t('orders.number'),
        render: (row) => <span className="font-mono text-xs">{row.order_no}</span>,
      },
      { key: 'plan_name', title: t('orders.plan'), render: (row) => row.plan_name ?? '—' },
      {
        key: 'period',
        title: t('orders.period'),
        render: (row) => t(`plans.period.${row.period}`, { defaultValue: row.period }),
      },
      {
        key: 'amount_cents',
        title: t('orders.amount'),
        render: (row) => formatBalanceCents(row.amount_cents),
      },
      {
        key: 'status',
        title: t('orders.status'),
        render: (row) => t(`orders.statusValue.${row.status}`, { defaultValue: row.status }),
      },
      {
        key: 'created_at',
        title: t('orders.createdAt'),
        render: (row) => formatDateTime(row.created_at),
      },
      {
        key: 'actions',
        title: t('common.actions'),
        render: (row) =>
          row.status === 'pending' ? (
            <button type="button" className="data-table-link-btn" onClick={() => navigate(`/orders/${row.id}/pay`)}>
              {t('orders.pay')}
            </button>
          ) : (
            '—'
          ),
      },
    ],
    [navigate, t],
  )

  return (
    <section className="page-container">
      <PageListHeader title={t('pages.orders')} />
      <EntityList
        columns={columns}
        queryKey={['user-orders']}
        fetchPage={(params) => getPage<OrderRow>('/v1/orders', params)}
        rowKey={(row) => row.id}
        emptyText={t('orders.empty')}
      />
    </section>
  )
}
