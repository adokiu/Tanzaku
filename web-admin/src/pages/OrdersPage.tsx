import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { EntityIdCell, ENTITY_LIST_COL_ID } from '@/components/EntityListLeadingCells'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { formatDateTime } from '@/utils/formatDateTime'
import { parseBalanceYuanInput } from '@/utils/formatMoney'

type OrderRow = {
  id: string
  user_id: string
  user_email: string
  plan_id: string | null
  plan_name: string
  kind: string
  period: string
  amount_cents: number
  status: string
  created_at: string
}

type UserOption = { id: string; email: string; role: string; status: string }
type PlanOption = { id: string; name: string; enabled: boolean; traffic_period: string }

const PERIODS = ['day', 'week', 'month', 'quarter', 'year', 'lifetime'] as const

function yuanLabel(cents: number) {
  const yuan = (Number.isFinite(cents) ? cents : 0) / 100
  const sign = yuan < 0 ? '-' : ''
  return `${sign}¥${Math.abs(yuan).toFixed(2)}`
}

export default function OrdersPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [q, setQ] = useState('')
  const [kind, setKind] = useState('')
  const [period, setPeriod] = useState('')
  const [status, setStatus] = useState('')
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [viewing, setViewing] = useState<OrderRow | null>(null)
  const [userId, setUserId] = useState('')
  const [planId, setPlanId] = useState('')
  const [amountYuan, setAmountYuan] = useState('0.00')
  const [expiresAtLocal, setExpiresAtLocal] = useState('')
  const [formError, setFormError] = useState('')

  const fetchOrders = useCallback(
    (params: { page: number; page_size: number }) =>
      getPage<OrderRow>('/v1/admin/orders', {
        ...params,
        q: q.trim() || undefined,
        kind: kind || undefined,
        period: period || undefined,
        status: status || undefined,
      }),
    [kind, period, q, status],
  )
  const users = useQuery({
    queryKey: ['admin-users-options'],
    queryFn: () => getPageItems<UserOption>('/v1/admin/users'),
    enabled: sidebarOpen && !viewing,
  })
  const plans = useQuery({
    queryKey: ['admin-plans-options'],
    queryFn: () => getPageItems<PlanOption>('/v1/admin/plans'),
    enabled: sidebarOpen && !viewing,
  })

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-orders'] })
    await queryClient.invalidateQueries({ queryKey: ['admin-users'] })
  }

  const create = useMutation({
    mutationFn: async () => {
      const amount_cents = parseBalanceYuanInput(amountYuan)
      if (amount_cents == null || amount_cents < 0) {
        throw new Error(t('orders.amountInvalid'))
      }
      const payload: { user_id: string; plan_id: string; amount_cents: number; expires_at?: string } = {
        user_id: userId,
        plan_id: planId,
        amount_cents,
      }
      if (expiresAtLocal.trim()) payload.expires_at = new Date(expiresAtLocal).toISOString()
      await apiClient.post('/v1/admin/orders', payload)
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      await invalidate()
    },
    onError: (error) => setFormError(error instanceof Error && error.message === t('orders.amountInvalid') ? error.message : translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/orders/${id}`)
    },
    onSuccess: async () => {
      setViewing(null)
      setSidebarOpen(false)
      await invalidate()
    },
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const userOptions = useMemo(
    () => (users.data ?? []).filter((user) => user.role === 'user' && user.status === 'active').map((user) => ({
      label: user.email,
      value: user.id,
    })),
    [users.data],
  )
  const planOptions = useMemo(
    () => (plans.data ?? []).filter((plan) => plan.enabled).map((plan) => ({ label: plan.name, value: plan.id })),
    [plans.data],
  )
  const kindOptions = [
    { label: t('orders.kindAll'), value: '' },
    { label: t('orders.kindNew'), value: 'new' },
    { label: t('orders.kindUpgrade'), value: 'upgrade' },
  ]
  const periodOptions = [
    { label: t('orders.periodAll'), value: '' },
    ...PERIODS.map((value) => ({ label: t(`orders.period.${value}`), value })),
  ]
  const statusOptions = [
    { label: t('orders.statusAll'), value: '' },
    { label: t('orders.statusCompleted'), value: 'completed' },
    { label: t('orders.statusCancelled'), value: 'cancelled' },
    { label: t('orders.statusCredited'), value: 'credited' },
  ]

  function kindLabel(value: string) {
    if (value === 'upgrade') return t('orders.kindUpgrade')
    if (value === 'new') return t('orders.kindNew')
    return value
  }
  function statusLabel(value: string) {
    if (value === 'completed') return t('orders.statusCompleted')
    if (value === 'cancelled') return t('orders.statusCancelled')
    if (value === 'credited') return t('orders.statusCredited')
    return value
  }
  function periodLabel(value: string) {
    return t(`orders.period.${value}`, { defaultValue: value })
  }

  function openCreate() {
    setViewing(null)
    setUserId('')
    setPlanId('')
    setAmountYuan('0.00')
    setExpiresAtLocal('')
    setFormError('')
    setSidebarOpen(true)
  }

  function openView(row: OrderRow) {
    setViewing(row)
    setFormError('')
    setSidebarOpen(true)
  }

  const columns: Column<OrderRow>[] = useMemo(() => [
    {
      key: 'id',
      title: t('orders.number'),
      width: ENTITY_LIST_COL_ID,
      fixedWidth: true,
      render: (row) => <EntityIdCell id={row.id} />,
    },
    {
      key: 'kind',
      title: t('orders.kind'),
      width: 88,
      render: (row) => <span className="data-table-cell">{kindLabel(row.kind)}</span>,
    },
    {
      key: 'plan_name',
      title: t('orders.plan'),
      render: (row) => <span className="data-table-cell" title={row.plan_name}>{row.plan_name}</span>,
    },
    {
      key: 'period',
      title: t('orders.periodLabel'),
      width: 96,
      render: (row) => <span className="data-table-cell">{periodLabel(row.period)}</span>,
    },
    {
      key: 'amount_cents',
      title: t('orders.amount'),
      width: 110,
      render: (row) => <span className="data-table-cell">{yuanLabel(row.amount_cents)}</span>,
    },
    {
      key: 'status',
      title: t('orders.status'),
      width: 96,
      render: (row) => <span className="data-table-cell">{statusLabel(row.status)}</span>,
    },
    {
      key: 'user_email',
      title: t('orders.user'),
      width: 180,
      render: (row) => <span className="data-table-cell" title={row.user_email}>{row.user_email}</span>,
    },
    {
      key: 'created_at',
      title: t('orders.createdAt'),
      width: 168,
      render: (row) => <span className="data-table-cell">{formatDateTime(row.created_at)}</span>,
    },
    {
      key: 'actions',
      title: t('orders.actions'),
      width: 120,
      render: (row) => (
        <div className="data-table-actions">
          <button type="button" className="data-table-link-btn" onClick={() => openView(row)}>
            {t('orders.view')}
          </button>
          <button
            type="button"
            className="data-table-link-btn data-table-link-btn--danger"
            onClick={() => {
              if (window.confirm(t('orders.deleteConfirm'))) remove.mutate(row.id)
            }}
          >
            {t('common.delete')}
          </button>
        </div>
      ),
    },
  ], [remove, t])

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.orders')}
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('forms.createOrder')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-orders'] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <Input
          value={q}
          onChange={(event) => setQ(event.target.value)}
          placeholder={t('orders.search')}
          style={{ minWidth: 220, flex: '1 1 220px' }}
        />
        <div style={{ width: 140 }}>
          <Select value={kind} options={kindOptions} onChange={(value) => setKind(String(value))} />
        </div>
        <div style={{ width: 160 }}>
          <Select value={period} options={periodOptions} onChange={(value) => setPeriod(String(value))} />
        </div>
        <div style={{ width: 160 }}>
          <Select value={status} options={statusOptions} onChange={(value) => setStatus(String(value))} />
        </div>
      </div>
      <EntityList
        columns={columns}
        queryKey={['admin-orders', q, kind, period, status]}
        fetchPage={fetchOrders}
        rowKey={(row) => row.id}
        resetPageWhen={`${q}|${kind}|${period}|${status}`}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={viewing ? t('orders.view') : t('forms.createOrder')}
        onClose={() => setSidebarOpen(false)}
        footer={viewing ? (
          <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
        ) : (
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button
              loading={create.isPending}
              onClick={() => {
                setFormError('')
                if (!userId || !planId) {
                  setFormError(t('orders.required'))
                  return
                }
                create.mutate()
              }}
            >
              {t('common.save')}
            </Button>
          </>
        )}
      >
        {viewing ? (
          <FormStack>
            <FormField label={t('orders.number')}><p className="text-sm">{viewing.id}</p></FormField>
            <FormField label={t('orders.user')}><p className="text-sm">{viewing.user_email}</p></FormField>
            <FormField label={t('orders.kind')}><p className="text-sm">{kindLabel(viewing.kind)}</p></FormField>
            <FormField label={t('orders.plan')}><p className="text-sm">{viewing.plan_name}</p></FormField>
            <FormField label={t('orders.periodLabel')}><p className="text-sm">{periodLabel(viewing.period)}</p></FormField>
            <FormField label={t('orders.amount')}><p className="text-sm">{yuanLabel(viewing.amount_cents)}</p></FormField>
            <FormField label={t('orders.status')}><p className="text-sm">{statusLabel(viewing.status)}</p></FormField>
            <FormField label={t('orders.createdAt')}><p className="text-sm">{formatDateTime(viewing.created_at)}</p></FormField>
          </FormStack>
        ) : (
          <FormStack>
            {formError ? <p className="sidebar-form-error" role="alert">{formError}</p> : null}
            <FormField label={t('orders.user')} required>
              <Select
                value={userId || undefined}
                options={userOptions}
                placeholder={t('orders.userPlaceholder')}
                searchable
                onChange={(value) => setUserId(String(value))}
              />
            </FormField>
            <FormField label={t('orders.plan')} required hint={t('orders.assignHint')}>
              <Select
                value={planId || undefined}
                options={planOptions}
                placeholder={t('orders.planPlaceholder')}
                searchable
                onChange={(value) => setPlanId(String(value))}
              />
            </FormField>
            <FormField label={t('orders.amount')} hint={t('orders.amountHint')}>
              <Input value={amountYuan} onChange={(event) => setAmountYuan(event.target.value)} inputMode="decimal" />
            </FormField>
            <FormField label={t('orders.expiresAt')} hint={t('orders.expiresHint')}>
              <Input type="datetime-local" value={expiresAtLocal} onChange={(event) => setExpiresAtLocal(event.target.value)} />
            </FormField>
          </FormStack>
        )}
      </GenericSidebar>
    </section>
  )
}
