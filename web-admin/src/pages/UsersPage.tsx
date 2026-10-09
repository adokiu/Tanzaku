import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { EntityIdCell, ENTITY_LIST_COL_ID } from '@/components/EntityListLeadingCells'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import {
  UserAccountStatusCell,
  UserRoleCell,
  UserSubscriptionCell,
  UserTrafficTotalCell,
  UserTrafficUsedCell,
} from '@/components/UserListCells'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime } from '@/utils/formatDateTime'
import {
  balanceYuanFromCents,
  formatBalanceCents,
  parseBalanceYuanInput,
} from '@/utils/formatMoney'
import { validatePassword } from '@/utils/passwordPolicy'
import {
  bytesToTrafficAmountDisplay,
  bytesToTrafficQuotaDisplay,
  trafficAmountToBytes,
  trafficQuotaToBytes,
  type TrafficAmountUnit,
  type TrafficQuotaUnit,
} from '@/utils/trafficQuota'
import { ENTITY_LIST_ACTIONS_COLUMN_PX } from '@/utils/entityListLayout'

type User = {
  id: string
  email: string
  role: string
  status: string
  last_login_at: string | null
  created_at: string
  subscription_plan_id: string | null
  subscription_plan_name: string | null
  subscription_status: string | null
  subscription_expires_at: string | null
  traffic_used_bytes: number
  traffic_quota_bytes: number | null
  subscription_speed_limit_mbps: number | null
  subscription_max_tunnels: number | null
  traffic_exhausted: boolean
  balance_cents: number
}

type Plan = { id: string; name: string; enabled: boolean }

const AMOUNT_UNITS: TrafficAmountUnit[] = ['MB', 'GB', 'TB', 'PB']
const QUOTA_UNITS: TrafficQuotaUnit[] = ['MB', 'GB', 'TB', 'PB', 'unlimited']

/** 订阅副本可编辑项（表单字符串态，用于和初始值比对，只提交改动项）。 */
type SubscriptionOverrides = {
  usedValue: string
  usedUnit: TrafficAmountUnit
  quotaValue: string
  quotaUnit: TrafficQuotaUnit
  speedLimit: string
  maxTunnels: string
}

const EMPTY_OVERRIDES: SubscriptionOverrides = {
  usedValue: '0',
  usedUnit: 'GB',
  quotaValue: '',
  quotaUnit: 'unlimited',
  speedLimit: '',
  maxTunnels: '',
}

function overridesFromUser(row: User): SubscriptionOverrides {
  const used = bytesToTrafficAmountDisplay(row.traffic_used_bytes)
  const quota = bytesToTrafficQuotaDisplay(row.traffic_quota_bytes)
  return {
    usedValue: used.value,
    usedUnit: used.unit,
    quotaValue: quota.value,
    quotaUnit: quota.unit,
    speedLimit: row.subscription_speed_limit_mbps == null ? '' : String(row.subscription_speed_limit_mbps),
    maxTunnels: row.subscription_max_tunnels == null ? '' : String(row.subscription_max_tunnels),
  }
}

/** 返回需提交的改动字段；输入非法时返回 null。 */
function overridesPayload(
  current: SubscriptionOverrides,
  initial: SubscriptionOverrides,
): Record<string, unknown> | null {
  const payload: Record<string, unknown> = {}
  if (current.usedValue !== initial.usedValue || current.usedUnit !== initial.usedUnit) {
    const used = trafficAmountToBytes(current.usedValue, current.usedUnit)
    if (used == null) return null
    payload.traffic_used_bytes = used
  }
  if (current.quotaValue !== initial.quotaValue || current.quotaUnit !== initial.quotaUnit) {
    if (current.quotaUnit === 'unlimited') {
      payload.traffic_quota_bytes = null
    } else {
      const quota = trafficQuotaToBytes(current.quotaValue, current.quotaUnit)
      if (quota == null) return null
      payload.traffic_quota_bytes = quota
    }
  }
  if (current.speedLimit !== initial.speedLimit) {
    const speed = Number(current.speedLimit)
    if (!current.speedLimit.trim() || !Number.isInteger(speed) || speed < 0) return null
    payload.speed_limit_mbps = speed
  }
  if (current.maxTunnels !== initial.maxTunnels) {
    const max = Number(current.maxTunnels)
    if (!current.maxTunnels.trim() || !Number.isInteger(max) || max < 0) return null
    payload.max_tunnels = max
  }
  return payload
}

type AccountRole = 'user' | 'admin'

function isoToDatetimeLocalValue(iso: string | null | undefined): string {
  if (!iso?.trim()) return ''
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

export default function UsersPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<AccountRole>('user')
  const [planId, setPlanId] = useState('')
  const [initialPlanId, setInitialPlanId] = useState('')
  const [initialExpiresLocal, setInitialExpiresLocal] = useState('')
  const [expiresAtLocal, setExpiresAtLocal] = useState('')
  const [balanceYuan, setBalanceYuan] = useState('0.00')
  const [overrides, setOverrides] = useState<SubscriptionOverrides>(EMPTY_OVERRIDES)
  const [initialOverrides, setInitialOverrides] = useState<SubscriptionOverrides>(EMPTY_OVERRIDES)
  const canEditOverrides = Boolean(editingId && initialPlanId && planId === initialPlanId)
  const planChanged = Boolean(editingId && initialPlanId && planId && planId !== initialPlanId)
  const updateOverride = <K extends keyof SubscriptionOverrides>(key: K, value: SubscriptionOverrides[K]) =>
    setOverrides((prev) => ({ ...prev, [key]: value }))
  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<User>('/v1/admin/users', params),
    [],
  )
  const plansQuery = useQuery({
    queryKey: ['admin-plans-options'],
    queryFn: () => getPageItems<Plan>('/v1/admin/plans'),
  })

  const planOptions = useMemo(
    () =>
      (plansQuery.data ?? [])
        .filter((plan) => plan.enabled)
        .map((plan) => ({ label: plan.name, value: plan.id })),
    [plansQuery.data],
  )

  const roleOptions = useMemo(
    () => [
      { label: t('users.roleUser'), value: 'user' },
      { label: t('users.roleAdmin'), value: 'admin' },
    ],
    [t],
  )

  const invalidate = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-users'] })
  }, [queryClient])

  const resetCreateForm = useCallback(() => {
    setEmail('')
    setPassword('')
    setRole('user')
    setPlanId('')
    setInitialPlanId('')
    setInitialExpiresLocal('')
    setExpiresAtLocal('')
    setBalanceYuan('0.00')
    setOverrides(EMPTY_OVERRIDES)
    setInitialOverrides(EMPTY_OVERRIDES)
  }, [])

  function balanceCentsFromForm(): number | null {
    return parseBalanceYuanInput(balanceYuan)
  }

  const create = useMutation({
    mutationFn: async () => {
      const balance_cents = balanceCentsFromForm()
      if (balance_cents == null) {
        throw new Error(t('users.balanceInvalid'))
      }
      const payload: Record<string, unknown> = {
        email: email.trim(),
        password,
        role,
        balance_cents,
      }
      if (planId) {
        payload.plan_id = planId
        if (expiresAtLocal.trim()) {
          payload.expires_at = new Date(expiresAtLocal).toISOString()
        }
      }
      return apiClient.post('/v1/admin/users', payload)
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      resetCreateForm()
      await invalidate()
      toast.success(t('common.createSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editingId) return
      const balance_cents = balanceCentsFromForm()
      if (balance_cents == null) {
        throw new Error(t('users.balanceInvalid'))
      }
      const payload: Record<string, unknown> = {
        email: email.trim(),
        role,
        balance_cents,
      }
      if (password.trim()) {
        payload.password = password
      }
      if (planId && planId !== initialPlanId) {
        payload.plan_id = planId
        if (expiresAtLocal.trim()) {
          payload.expires_at = new Date(expiresAtLocal).toISOString()
        }
      } else if (
        initialPlanId
        && expiresAtLocal.trim()
        && expiresAtLocal !== initialExpiresLocal
      ) {
        payload.expires_at = new Date(expiresAtLocal).toISOString()
      }
      if (canEditOverrides) {
        const changed = overridesPayload(overrides, initialOverrides)
        if (changed == null) {
          throw new Error(t('users.overridesInvalid'))
        }
        Object.assign(payload, changed)
      }
      await apiClient.put(`/v1/admin/users/${editingId}`, payload)
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      resetCreateForm()
      await invalidate()
      toast.success(t('common.saveSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const setStatus = useMutation({
    mutationFn: async ({ id, status }: { id: string; status: 'active' | 'disabled' }) => {
      await apiClient.patch(`/v1/admin/users/${id}/status`, { status })
    },
    onSuccess: async () => {
      await invalidate()
      toast.success(t('common.operationSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/users/${id}`)
    },
    onSuccess: async () => {
      await invalidate()
      toast.success(t('common.deleteSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const openCreate = useCallback(() => {
    setEditingId(null)
    resetCreateForm()
    setSidebarOpen(true)
  }, [resetCreateForm])

  const openEdit = useCallback((row: User) => {
    setEditingId(row.id)
    setEmail(row.email)
    setPassword('')
    setRole(row.role === 'admin' ? 'admin' : 'user')
    const currentPlanId = row.subscription_plan_id ?? ''
    const expiresLocal = isoToDatetimeLocalValue(row.subscription_expires_at)
    setInitialPlanId(currentPlanId)
    setPlanId(currentPlanId)
    setInitialExpiresLocal(expiresLocal)
    setExpiresAtLocal(expiresLocal)
    setBalanceYuan(balanceYuanFromCents(row.balance_cents))
    const rowOverrides = overridesFromUser(row)
    setOverrides(rowOverrides)
    setInitialOverrides(rowOverrides)
    setSidebarOpen(true)
  }, [])

  const columns = useMemo((): Column<User>[] => {
    return [
      {
        key: 'id',
        title: t('fields.id'),
        width: ENTITY_LIST_COL_ID,
        fixedWidth: true,
        render: (row) => <EntityIdCell id={row.id} />,
      },
      {
        key: 'email',
        title: translateField(t, 'email'),
        render: (row) => (
          <CellTooltip tip={row.email} className="entity-list-cell entity-list-cell--name data-table-cell">
            {row.email}
          </CellTooltip>
        ),
      },
      {
        key: 'status',
        title: translateField(t, 'status'),
        width: 76,
        fixedWidth: true,
        render: (row) => <UserAccountStatusCell status={row.status} />,
      },
      {
        key: 'role',
        title: translateField(t, 'role'),
        width: 96,
        render: (row) => <UserRoleCell role={row.role} />,
      },
      {
        key: 'subscription',
        title: translateField(t, 'subscription'),
        width: 128,
        render: (row) => (
          <UserSubscriptionCell
            planName={row.subscription_plan_name}
            subscriptionStatus={row.subscription_status}
          />
        ),
      },
      {
        key: 'traffic_used',
        title: translateField(t, 'traffic_used'),
        width: 128,
        render: (row) => (
          <UserTrafficUsedCell
            usedBytes={row.traffic_used_bytes}
            quotaBytes={row.traffic_quota_bytes}
          />
        ),
      },
      {
        key: 'traffic_total',
        title: translateField(t, 'traffic_total'),
        width: 96,
        render: (row) => <UserTrafficTotalCell quotaBytes={row.traffic_quota_bytes} />,
      },
      {
        key: 'expires_at',
        title: translateField(t, 'expires_at'),
        width: 168,
        render: (row) => (
          <span className="data-table-cell">
            {row.subscription_expires_at ? formatDateTime(row.subscription_expires_at) : '—'}
          </span>
        ),
      },
      {
        key: 'balance',
        title: translateField(t, 'balance'),
        width: 88,
        render: (row) => (
          <span className="data-table-cell tabular-nums">{formatBalanceCents(row.balance_cents)}</span>
        ),
      },
      {
        key: 'created_at',
        title: translateField(t, 'created_at'),
        width: 168,
        render: (row) => (
          <span className="data-table-cell">{formatDateTime(row.created_at)}</span>
        ),
      },
      {
        key: 'actions',
        title: translateField(t, 'action'),
        width: ENTITY_LIST_ACTIONS_COLUMN_PX,
        render: (row) => (
          <div className="data-table-actions">
            <button type="button" className="data-table-link-btn" onClick={() => openEdit(row)}>
              {t('common.edit')}
            </button>
            <button
              type="button"
              className="data-table-link-btn"
              onClick={() =>
                setStatus.mutate({
                  id: row.id,
                  status: row.status === 'active' ? 'disabled' : 'active',
                })
              }
            >
              {row.status === 'active' ? t('common.disable') : t('common.enable')}
            </button>
            <button
              type="button"
              className="data-table-link-btn data-table-link-btn--danger"
              onClick={() => {
                if (window.confirm(t('users.deleteConfirm', { email: row.email }))) {
                  remove.mutate(row.id)
                }
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ]
  }, [openEdit, remove.mutate, setStatus.mutate, t])

  function submitSidebar() {
    if (!email.trim()) {
      toast.error(t('users.emailRequired'))
      return
    }
    if (balanceCentsFromForm() == null) {
      toast.error(t('users.balanceInvalid'))
      return
    }
    if (!editingId) {
      const passwordError = validatePassword(password)
      if (passwordError) {
        toast.error(t(passwordError))
        return
      }
      create.mutate()
      return
    }
    if (password.trim()) {
      const passwordError = validatePassword(password)
      if (passwordError) {
        toast.error(t(passwordError))
        return
      }
    }
    if (canEditOverrides && overridesPayload(overrides, initialOverrides) == null) {
      toast.error(t('users.overridesInvalid'))
      return
    }
    update.mutate()
  }

  const sidebarSaving = editingId ? update.isPending : create.isPending

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('forms.createUser')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-users'] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        listLayoutId="users"
        columns={columns}
        queryKey={['admin-users']}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('users.editUser') : t('forms.createUser')}
        onClose={() => setSidebarOpen(false)}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={sidebarSaving} onClick={submitSidebar}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          <FormField label={translateField(t, 'user_email')} required>
            <Input type="email" value={email} onChange={(event) => setEmail(event.target.value)} required />
          </FormField>
          <FormField
            label={translateField(t, 'initial_password')}
            required={!editingId}
            hint={editingId ? t('users.passwordOptionalOnEdit') : t('passwordPolicy.hint')}
          >
            <Input
              type="password"
              autoComplete="new-password"
              minLength={editingId ? undefined : 6}
              maxLength={32}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required={!editingId}
            />
          </FormField>
          <FormField label={translateField(t, 'role')} required>
            <Select value={role} options={roleOptions} onChange={(value) => setRole(value === 'admin' ? 'admin' : 'user')} />
          </FormField>
          <FormField label={translateField(t, 'balance')} required hint={t('users.balanceFormHint')}>
            <Input
              type="number"
              min={0}
              step={0.01}
              inputMode="decimal"
              value={balanceYuan}
              onChange={(event) => setBalanceYuan(event.target.value)}
              required
            />
          </FormField>
          <FormField label={t('users.subscriptionPlan')}>
            <Select
              value={planId || undefined}
              options={planOptions}
              placeholder={t('select.plan')}
              onChange={(value) => setPlanId(String(value))}
            />
          </FormField>
          <FormField label={translateField(t, 'expires_at')}>
            <Input
              type="datetime-local"
              value={expiresAtLocal}
              disabled={!planId && !initialPlanId}
              onChange={(event) => setExpiresAtLocal(event.target.value)}
            />
          </FormField>
          {planChanged ? (
            <p className="text-sm text-apple-secondary">{t('users.overridesAfterPlanChange')}</p>
          ) : null}
          {canEditOverrides ? (
            <>
              <p className="apple-label">{t('users.subscriptionOverrides')}</p>
              <FormField label={t('users.trafficUsedLabel')} hint={t('users.trafficUsedHint')}>
                <div className="plans-traffic-quota-row">
                  <Input
                    type="number"
                    min={0}
                    step="any"
                    value={overrides.usedValue}
                    onChange={(event) => updateOverride('usedValue', event.target.value)}
                  />
                  <Select
                    value={overrides.usedUnit}
                    options={AMOUNT_UNITS.map((unit) => ({ label: unit, value: unit }))}
                    onChange={(value) => updateOverride('usedUnit', String(value) as TrafficAmountUnit)}
                  />
                </div>
              </FormField>
              <FormField label={t('users.trafficQuotaLabel')}>
                <div className="plans-traffic-quota-row">
                  <Input
                    type="number"
                    min={0}
                    step="any"
                    value={overrides.quotaUnit === 'unlimited' ? '' : overrides.quotaValue}
                    disabled={overrides.quotaUnit === 'unlimited'}
                    onChange={(event) => updateOverride('quotaValue', event.target.value)}
                  />
                  <Select
                    value={overrides.quotaUnit}
                    options={QUOTA_UNITS.map((unit) => ({
                      label: unit === 'unlimited' ? t('users.trafficUnlimited') : unit,
                      value: unit,
                    }))}
                    onChange={(value) => {
                      const unit = String(value) as TrafficQuotaUnit
                      setOverrides((prev) => ({
                        ...prev,
                        quotaUnit: unit,
                        quotaValue: unit === 'unlimited' ? '' : prev.quotaValue,
                      }))
                    }}
                  />
                </div>
              </FormField>
              <FormField label={t('users.speedLimitLabel')} hint={t('users.speedLimitHint')}>
                <Input
                  type="number"
                  min={0}
                  step={1}
                  value={overrides.speedLimit}
                  onChange={(event) => updateOverride('speedLimit', event.target.value)}
                />
              </FormField>
              <FormField label={t('users.maxTunnelsLabel')}>
                <Input
                  type="number"
                  min={0}
                  step={1}
                  value={overrides.maxTunnels}
                  onChange={(event) => updateOverride('maxTunnels', event.target.value)}
                />
              </FormField>
            </>
          ) : null}
        </FormStack>
      </GenericSidebar>
    </section>
  )
}
