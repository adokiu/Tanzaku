import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { translateField } from '@/i18n/fieldLabel'
import { balanceYuanFromCents } from '@/utils/formatMoney'
import {
  bytesToTrafficQuotaDisplay,
  formatTrafficQuotaLabel,
  trafficQuotaToBytes,
  type TrafficQuotaUnit,
} from '@/utils/trafficQuota'

type NodeGroup = { id: string; name: string; enabled: boolean }

type Plan = {
  id: string
  name: string
  description: string
  speed_limit_mbps: number
  max_conns_per_tunnel: number
  max_new_conns_per_sec: number
  max_tunnels: number
  allow_custom_port: boolean
  traffic_quota_bytes: number | null
  traffic_period: string
  allowed_protocols: string[]
  traffic_count_mode: string
  enabled: boolean
  price_month_cents: number | null
  price_quarter_cents: number | null
  price_half_year_cents: number | null
  price_year_cents: number | null
  price_two_year_cents: number | null
  price_three_year_cents: number | null
  price_traffic_pack_cents: number | null
  price_reset_pack_cents: number | null
  node_group_ids: string[]
  node_group_names: string[]
}

type PlanForm = {
  name: string
  description: string
  speed_limit_mbps: string
  max_conns_per_tunnel: string
  max_new_conns_per_sec: string
  max_tunnels: string
  allow_custom_port: boolean
  traffic_quota_value: string
  traffic_quota_unit: TrafficQuotaUnit
  traffic_period: string
  traffic_count_mode: string
  price_base: string
  price_month: string
  price_quarter: string
  price_half_year: string
  price_year: string
  price_two_year: string
  price_three_year: string
  price_traffic_pack: string
  price_reset_pack: string
}

const initialForm: PlanForm = {
  name: '',
  description: '',
  speed_limit_mbps: '',
  max_conns_per_tunnel: '',
  max_new_conns_per_sec: '',
  max_tunnels: '',
  allow_custom_port: false,
  traffic_quota_value: '',
  traffic_quota_unit: 'GB',
  traffic_period: 'system',
  traffic_count_mode: 'sum',
  price_base: '',
  price_month: '',
  price_quarter: '',
  price_half_year: '',
  price_year: '',
  price_two_year: '',
  price_three_year: '',
  price_traffic_pack: '',
  price_reset_pack: '',
}

const periods = ['system', 'month_first', 'month_purchase', 'never', 'year_first', 'year_purchase'] as const
const trafficUnits: TrafficQuotaUnit[] = ['MB', 'GB', 'TB', 'PB', 'unlimited']
const countModes = ['sum', 'inbound', 'outbound', 'max'] as const
/** 基础价（月付单价）仅前端用；按月数倍率预填各周期，仍可改。 */
const CYCLE_PRICE_FIELDS = [
  ['price_month', 'month', 1],
  ['price_quarter', 'quarter', 3],
  ['price_half_year', 'half_year', 6],
  ['price_year', 'year', 12],
  ['price_two_year', 'two_year', 24],
  ['price_three_year', 'three_year', 36],
] as const
const ADDON_PRICE_FIELDS = [
  ['price_traffic_pack', 'traffic_pack'],
  ['price_reset_pack', 'reset_pack'],
] as const

function yuanFromMonths(baseYuan: number, months: number) {
  return balanceYuanFromCents(Math.round(baseYuan * months * 100))
}

function applyBasePrice(baseRaw: string): Pick<
  PlanForm,
  'price_base' | 'price_month' | 'price_quarter' | 'price_half_year' | 'price_year' | 'price_two_year' | 'price_three_year'
> {
  const trimmed = baseRaw.trim()
  if (!trimmed) {
    return {
      price_base: baseRaw,
      price_month: '',
      price_quarter: '',
      price_half_year: '',
      price_year: '',
      price_two_year: '',
      price_three_year: '',
    }
  }
  const yuan = Number(trimmed)
  if (!Number.isFinite(yuan) || yuan < 0) {
    return { price_base: baseRaw, price_month: '', price_quarter: '', price_half_year: '', price_year: '', price_two_year: '', price_three_year: '' }
  }
  return {
    price_base: baseRaw,
    price_month: yuanFromMonths(yuan, 1),
    price_quarter: yuanFromMonths(yuan, 3),
    price_half_year: yuanFromMonths(yuan, 6),
    price_year: yuanFromMonths(yuan, 12),
    price_two_year: yuanFromMonths(yuan, 24),
    price_three_year: yuanFromMonths(yuan, 36),
  }
}

const LEGACY_RESET: Record<string, string> = {
  day: 'month_purchase',
  week: 'month_purchase',
  month: 'month_purchase',
  quarter: 'month_purchase',
  year: 'year_first',
  lifetime: 'never',
}

function normalizeResetMode(value: string) {
  if ((periods as readonly string[]).includes(value)) return value
  return LEGACY_RESET[value] ?? 'system'
}

function planToForm(plan: Plan): PlanForm {
  const traffic = bytesToTrafficQuotaDisplay(plan.traffic_quota_bytes)
  return {
    name: plan.name,
    description: plan.description ?? '',
    speed_limit_mbps: String(plan.speed_limit_mbps),
    max_conns_per_tunnel: String(plan.max_conns_per_tunnel),
    max_new_conns_per_sec: String(plan.max_new_conns_per_sec),
    max_tunnels: String(plan.max_tunnels),
    allow_custom_port: plan.allow_custom_port,
    traffic_quota_value: traffic.value,
    traffic_quota_unit: traffic.unit,
    traffic_period: normalizeResetMode(plan.traffic_period),
    traffic_count_mode: plan.traffic_count_mode || 'sum',
    // 基础价不落库：编辑时用月付回填，方便再按倍率改
    price_base: yuanFromCents(plan.price_month_cents),
    price_month: yuanFromCents(plan.price_month_cents),
    price_quarter: yuanFromCents(plan.price_quarter_cents),
    price_half_year: yuanFromCents(plan.price_half_year_cents),
    price_year: yuanFromCents(plan.price_year_cents),
    price_two_year: yuanFromCents(plan.price_two_year_cents),
    price_three_year: yuanFromCents(plan.price_three_year_cents),
    price_traffic_pack: yuanFromCents(plan.price_traffic_pack_cents),
    price_reset_pack: yuanFromCents(plan.price_reset_pack_cents),
  }
}

function yuanFromCents(cents: number | null | undefined) {
  return cents == null ? '' : balanceYuanFromCents(cents)
}

function parseOptionalYuan(value: string): number | null {
  const trimmed = value.trim()
  if (!trimmed) return null
  const yuan = Number(trimmed)
  if (!Number.isFinite(yuan) || yuan < 0) {
    throw new Error('invalid_price')
  }
  return Math.round(yuan * 100)
}

function buildPlanPayload(
  form: PlanForm,
  allowedProtocols: string[],
  nodeGroupIds: string[],
) {
  const traffic_quota_bytes = trafficQuotaToBytes(form.traffic_quota_value, form.traffic_quota_unit)
  if (form.traffic_quota_unit !== 'unlimited' && form.traffic_quota_value.trim() && traffic_quota_bytes == null) {
    throw new Error('invalid_traffic')
  }
  return {
    name: form.name.trim(),
    description: form.description,
    speed_limit_mbps: Number(form.speed_limit_mbps),
    max_conns_per_tunnel: Number(form.max_conns_per_tunnel),
    max_new_conns_per_sec: Number(form.max_new_conns_per_sec),
    max_tunnels: Number(form.max_tunnels),
    allow_custom_port: form.allow_custom_port,
    traffic_quota_bytes,
    traffic_period: form.traffic_period,
    allowed_protocols: allowedProtocols,
    traffic_count_mode: form.traffic_count_mode,
    node_group_ids: nodeGroupIds,
    price_month_cents: parseOptionalYuan(form.price_month),
    price_quarter_cents: parseOptionalYuan(form.price_quarter),
    price_half_year_cents: parseOptionalYuan(form.price_half_year),
    price_year_cents: parseOptionalYuan(form.price_year),
    price_two_year_cents: parseOptionalYuan(form.price_two_year),
    price_three_year_cents: parseOptionalYuan(form.price_three_year),
    price_traffic_pack_cents: parseOptionalYuan(form.price_traffic_pack),
    price_reset_pack_cents: parseOptionalYuan(form.price_reset_pack),
  }
}

export default function PlansPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState(initialForm)
  const [allowedProtocols, setAllowedProtocols] = useState<string[]>(['tcp'])
  const [nodeGroupIds, setNodeGroupIds] = useState<string[]>([])
  const [groupPick, setGroupPick] = useState<string>('')
  const catalog = useQuery({
    queryKey: ['admin-dataplane'],
    queryFn: async () => (await apiClient.get('/v1/admin/dataplane')).data as { protocols: string[] },
  })
  const protocols = catalog.data?.protocols ?? []

  const fetchPlans = useCallback(
    (params: { page: number; page_size: number }) => getPage<Plan>('/v1/admin/plans', params),
    [],
  )
  const nodeGroups = useQuery({
    queryKey: ['admin-node-groups'],
    queryFn: () => getPageItems<NodeGroup>('/v1/admin/node-groups'),
  })

  const enabledGroups = useMemo(
    () => (nodeGroups.data ?? []).filter((group) => group.enabled),
    [nodeGroups.data],
  )
  const groupNameById = useMemo(() => {
    const map = new Map<string, string>()
    for (const group of nodeGroups.data ?? []) {
      map.set(group.id, group.name)
    }
    return map
  }, [nodeGroups.data])

  const groupSelectOptions = useMemo(
    () =>
      enabledGroups
        .filter((group) => !nodeGroupIds.includes(group.id))
        .map((group) => ({ label: group.name, value: group.id })),
    [enabledGroups, nodeGroupIds],
  )

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-plans'] })
  }

  const create = useMutation({
    mutationFn: async () => {
      const payload = buildPlanPayload(form, allowedProtocols, nodeGroupIds)
      return (await apiClient.post('/v1/admin/plans', payload)).data
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      setForm(initialForm)
      setAllowedProtocols(['tcp'])
      setNodeGroupIds([])
      setGroupPick('')
      await invalidate()
      toast.success(t('common.createSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editingId) return
      const payload = buildPlanPayload(form, allowedProtocols, nodeGroupIds)
      return (await apiClient.put(`/v1/admin/plans/${editingId}`, payload)).data
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      setForm(initialForm)
      setAllowedProtocols(['tcp'])
      setNodeGroupIds([])
      setGroupPick('')
      await invalidate()
      toast.success(t('common.saveSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      await apiClient.patch(`/v1/admin/plans/${id}/enabled`, { enabled })
    },
    onSuccess: async () => {
      await invalidate()
      toast.success(t('common.operationSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/plans/${id}`)
    },
    onSuccess: async () => {
      await invalidate()
      toast.success(t('common.deleteSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  function updateField<K extends keyof PlanForm>(key: K, value: PlanForm[K]) {
    if (key === 'price_base' && typeof value === 'string') {
      setForm((current) => ({ ...current, ...applyBasePrice(value) }))
      return
    }
    setForm((current) => ({ ...current, [key]: value }))
  }

  function toggleProtocol(value: string) {
    setAllowedProtocols((current) =>
      current.includes(value) ? current.filter((item) => item !== value) : [...current, value],
    )
  }

  function addNodeGroup(id: string) {
    if (!id || nodeGroupIds.includes(id)) return
    setNodeGroupIds((current) => [...current, id])
    setGroupPick('')
  }

  function removeNodeGroup(id: string) {
    setNodeGroupIds((current) => current.filter((item) => item !== id))
  }

  const openCreate = useCallback(() => {
    setEditingId(null)
    setForm(initialForm)
    setAllowedProtocols(catalog.data?.protocols ?? [])
    setNodeGroupIds([])
    setGroupPick('')
    setSidebarOpen(true)
  }, [catalog.data?.protocols])

  const openEdit = useCallback((row: Plan) => {
    setEditingId(row.id)
    setForm(planToForm(row))
    setAllowedProtocols(row.allowed_protocols?.length ? [...row.allowed_protocols] : ['tcp'])
    setNodeGroupIds(row.node_group_ids?.length ? [...row.node_group_ids] : [])
    setGroupPick('')
    setSidebarOpen(true)
  }, [])

  function submitForm() {
    if (!form.name.trim()) {
      toast.error(t('plans.nameRequired'))
      return
    }
    if (nodeGroupIds.length === 0) {
      toast.error(t('messages.nodeGroupRequired'))
      return
    }
    if (allowedProtocols.length === 0) {
      toast.error(t('plans.protocolRequired'))
      return
    }
    try {
      buildPlanPayload(form, allowedProtocols, nodeGroupIds)
    } catch (error) {
      toast.error(error instanceof Error && error.message === 'invalid_price' ? t('plans.priceInvalid') : t('plans.trafficQuotaInvalid'))
      return
    }
    if (editingId) {
      update.mutate()
    } else {
      create.mutate()
    }
  }

  const saving = create.isPending || update.isPending
  const unlimitedLabel = t('users.trafficUnlimited')

  const columns: Column<Plan>[] = useMemo(
    () => [
      {
        key: 'name',
        title: translateField(t, 'plan_name'),
        width: 160,
        render: (row) => (
          <CellTooltip tip={row.description?.trim() || undefined} className="data-table-cell">
            {row.name}
          </CellTooltip>
        ),
      },
      {
        key: 'node_groups',
        title: t('plans.serverGroups'),
        width: 140,
        render: (row) => (
          <CellTooltip tip={(row.node_group_names ?? []).join(', ') || undefined} className="data-table-cell">
            {(row.node_group_names ?? []).length
              ? row.node_group_names.join('、')
              : '—'}
          </CellTooltip>
        ),
      },
      {
        key: 'traffic_quota',
        title: t('plans.trafficQuotaColumn'),
        width: 120,
        render: (row) => (
          <span className="data-table-cell">
            {formatTrafficQuotaLabel(row.traffic_quota_bytes, unlimitedLabel)}
          </span>
        ),
      },
      {
        key: 'traffic_period',
        title: t('plans.billingCycleColumn'),
        width: 140,
        render: (row) => (
          <span className="data-table-cell">{t(`enums.billingPeriod.${normalizeResetMode(row.traffic_period)}`)}</span>
        ),
      },
      {
        key: 'speed_limit_mbps',
        title: translateField(t, 'speed_limit_mbps'),
        width: 88,
        render: (row) => (
          <span className="data-table-cell">{row.speed_limit_mbps} Mbps</span>
        ),
      },
      {
        key: 'max_tunnels',
        title: translateField(t, 'max_tunnels'),
        width: 72,
        render: (row) => <span className="data-table-cell">{row.max_tunnels}</span>,
      },
      {
        key: 'enabled',
        title: translateField(t, 'enabled'),
        width: 72,
        render: (row) => (
          <span className="data-table-cell">
            {row.enabled ? t('common.enabled') : t('common.disabled')}
          </span>
        ),
      },
      {
        key: 'actions',
        title: translateField(t, 'action'),
        render: (row) => (
          <div className="data-table-actions">
            <button type="button" className="data-table-link-btn" onClick={() => openEdit(row)}>
              {t('common.edit')}
            </button>
            <button
              type="button"
              className="data-table-link-btn"
              onClick={() => setEnabled.mutate({ id: row.id, enabled: !row.enabled })}
            >
              {row.enabled ? t('common.disable') : t('common.enable')}
            </button>
            <button
              type="button"
              className="data-table-link-btn data-table-link-btn--danger"
              onClick={() => {
                if (window.confirm(t('plans.deleteConfirm', { name: row.name }))) {
                  remove.mutate(row.id)
                }
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ],
    [openEdit, remove.mutate, setEnabled.mutate, t, unlimitedLabel],
  )

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        actions={(
          <>
            <Button
              size="sm"
              onClick={openCreate}
              disabled={nodeGroups.isPending || enabledGroups.length === 0}
            >
              <Plus size={16} />
              {t('forms.createPlan')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-plans'] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        listLayoutId="plans"
        columns={columns}
        queryKey={['admin-plans']}
        fetchPage={fetchPlans}
        rowKey={(row) => row.id}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('forms.editPlan') : t('forms.createPlan')}
        onClose={() => setSidebarOpen(false)}
        initialWidth={720}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saving} onClick={submitForm}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          <FormField label={t('forms.planName')} required>
            <Input
              value={form.name}
              onChange={(event) => updateField('name', event.target.value)}
              maxLength={100}
              placeholder={t('plans.namePlaceholder')}
              required
            />
          </FormField>
          <div className="plans-price-block">
            <FormField label={t('plans.priceSection')} hint={t('plans.priceHint')}>
              <label className="plans-price-field plans-price-field--base">
                <span>{t('plans.price.base')}</span>
                <span className="plans-price-input">
                  <span className="plans-price-yen">¥</span>
                  <Input
                    type="number"
                    min={0}
                    step="0.01"
                    value={form.price_base}
                    onChange={(event) => updateField('price_base', event.target.value)}
                    placeholder={t('plans.priceBasePlaceholder')}
                  />
                </span>
                <em>{t('plans.priceBaseHint')}</em>
              </label>
              <div className="plans-price-grid">
                {CYCLE_PRICE_FIELDS.map(([field, key]) => (
                  <label key={field} className="plans-price-field">
                    <span>{t(`plans.price.${key}`)}</span>
                    <span className="plans-price-input">
                      <span className="plans-price-yen">¥</span>
                      <Input
                        type="number"
                        min={0}
                        step="0.01"
                        value={form[field]}
                        onChange={(event) => updateField(field, event.target.value)}
                        placeholder={t('plans.pricePlaceholder')}
                      />
                    </span>
                  </label>
                ))}
                {ADDON_PRICE_FIELDS.map(([field, key]) => (
                  <label key={field} className="plans-price-field">
                    <span>{t(`plans.price.${key}`)}</span>
                    <span className="plans-price-input">
                      <span className="plans-price-yen">¥</span>
                      <Input
                        type="number"
                        min={0}
                        step="0.01"
                        value={form[field]}
                        onChange={(event) => updateField(field, event.target.value)}
                        placeholder={t('plans.pricePlaceholder')}
                      />
                    </span>
                    {key === 'traffic_pack' ? (
                      <em>{t('plans.priceTrafficPackHint')}</em>
                    ) : (
                      <em>{t('plans.priceResetPackHint')}</em>
                    )}
                  </label>
                ))}
              </div>
            </FormField>
          </div>
          <FormField label={t('plans.serverGroups')} required hint={t('plans.serverGroupsHint')}>
            <Select
              value={groupPick || undefined}
              options={groupSelectOptions}
              placeholder={t('plans.serverGroupsPlaceholder')}
              onChange={(value) => addNodeGroup(String(value))}
            />
            {nodeGroupIds.length > 0 ? (
              <ul className="plans-node-group-tags">
                {nodeGroupIds.map((id) => (
                  <li key={id} className="plans-node-group-tag">
                    <span>{groupNameById.get(id) ?? id}</span>
                    <button type="button" className="plans-node-group-tag__remove" aria-label={t('common.remove')} onClick={() => removeNodeGroup(id)}>
                      <X size={14} />
                    </button>
                  </li>
                ))}
              </ul>
            ) : null}
          </FormField>
          <FormField label={t('plans.billingCycleLabel')} required hint={t('plans.billingCycleHint')}>
            <Select
              value={normalizeResetMode(form.traffic_period)}
              options={periods.map((period) => ({
                label: t(`enums.billingPeriod.${period}`),
                value: period,
              }))}
              onChange={(value) => updateField('traffic_period', String(value))}
            />
          </FormField>
          <FormField label={t('plans.trafficQuotaLabel')} hint={t('plans.trafficQuotaHint')}>
            <div className="plans-traffic-quota-row">
              <Input
                type="number"
                min={0}
                step="any"
                value={form.traffic_quota_unit === 'unlimited' ? '' : form.traffic_quota_value}
                onChange={(event) => updateField('traffic_quota_value', event.target.value)}
                placeholder={t('plans.trafficQuotaPlaceholder')}
                disabled={form.traffic_quota_unit === 'unlimited'}
              />
              <Select
                value={form.traffic_quota_unit}
                options={trafficUnits.map((unit) => ({
                  label: unit === 'unlimited' ? t('users.trafficUnlimited') : unit,
                  value: unit,
                }))}
                onChange={(value) => {
                  const unit = String(value) as TrafficQuotaUnit
                  updateField('traffic_quota_unit', unit)
                  if (unit === 'unlimited') {
                    updateField('traffic_quota_value', '')
                  }
                }}
              />
            </div>
          </FormField>
          <FormField label={translateField(t, 'speed_limit_mbps')} required>
            <Input
              type="number"
              min={1}
              value={form.speed_limit_mbps}
              onChange={(event) => updateField('speed_limit_mbps', event.target.value)}
              placeholder={t('plans.speedPlaceholder')}
              required
            />
          </FormField>
          <FormField label={translateField(t, 'traffic_count_mode')} hint={t('plans.trafficCountModeHint')}>
            <Select
              value={form.traffic_count_mode}
              options={countModes.map((mode) => ({
                label: t(`enums.trafficCountMode.${mode}`),
                value: mode,
              }))}
              onChange={(value) => updateField('traffic_count_mode', String(value))}
            />
          </FormField>
          <FormField label={translateField(t, 'max_conns_per_tunnel')} required>
            <Input
              type="number"
              min={1}
              value={form.max_conns_per_tunnel}
              onChange={(event) => updateField('max_conns_per_tunnel', event.target.value)}
              required
            />
          </FormField>
          <FormField label={translateField(t, 'max_new_conns_per_sec')} required>
            <Input
              type="number"
              min={1}
              value={form.max_new_conns_per_sec}
              onChange={(event) => updateField('max_new_conns_per_sec', event.target.value)}
              required
            />
          </FormField>
          <FormField label={translateField(t, 'max_tunnels')} required>
            <Input
              type="number"
              min={1}
              value={form.max_tunnels}
              onChange={(event) => updateField('max_tunnels', event.target.value)}
              required
            />
          </FormField>
          <FormField label={t('forms.allowedProtocolsLegend')} required>
            <div className="flex flex-wrap gap-4">
              {protocols.map((protocol) => (
                <label className="flex items-center gap-2 text-sm" key={protocol}>
                  <input
                    type="checkbox"
                    checked={allowedProtocols.includes(protocol)}
                    onChange={() => toggleProtocol(protocol)}
                  />
                  {t(`enums.protocol.${protocol}`, { defaultValue: protocol.toUpperCase() })}
                </label>
              ))}
            </div>
          </FormField>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.allow_custom_port}
              onChange={(event) => updateField('allow_custom_port', event.target.checked)}
            />
            {t('forms.allowCustomPortUser')}
          </label>
          <FormField label={t('plans.descriptionLabel')} hint={t('plans.descriptionHint')}>
            <textarea
              className="input-field min-h-32 w-full text-sm"
              value={form.description}
              onChange={(event) => updateField('description', event.target.value)}
              placeholder={t('plans.descriptionPlaceholder')}
            />
          </FormField>
        </FormStack>
      </GenericSidebar>
    </section>
  )
}
