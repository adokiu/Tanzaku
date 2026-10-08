import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'
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
  traffic_period: 'month',
  traffic_count_mode: 'sum',
}

const periods = ['day', 'week', 'month', 'quarter', 'year', 'lifetime'] as const
const trafficUnits: TrafficQuotaUnit[] = ['MB', 'GB', 'TB', 'PB', 'unlimited']
const countModes = ['sum', 'inbound', 'outbound', 'max'] as const

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
    traffic_period: plan.traffic_period,
    traffic_count_mode: plan.traffic_count_mode || 'sum',
  }
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
  const [formError, setFormError] = useState('')

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
      setFormError('')
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
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
      setFormError('')
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      await apiClient.patch(`/v1/admin/plans/${id}/enabled`, { enabled })
    },
    onSuccess: invalidate,
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/plans/${id}`)
    },
    onSuccess: invalidate,
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  function updateField<K extends keyof PlanForm>(key: K, value: PlanForm[K]) {
    setForm((current) => ({ ...current, [key]: value }))
    setFormError('')
  }

  function toggleProtocol(value: string) {
    setAllowedProtocols((current) =>
      current.includes(value) ? current.filter((item) => item !== value) : [...current, value],
    )
    setFormError('')
  }

  function addNodeGroup(id: string) {
    if (!id || nodeGroupIds.includes(id)) return
    setNodeGroupIds((current) => [...current, id])
    setGroupPick('')
    setFormError('')
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
    setFormError('')
    setSidebarOpen(true)
  }, [catalog.data?.protocols])

  const openEdit = useCallback((row: Plan) => {
    setEditingId(row.id)
    setForm(planToForm(row))
    setAllowedProtocols(row.allowed_protocols?.length ? [...row.allowed_protocols] : ['tcp'])
    setNodeGroupIds(row.node_group_ids?.length ? [...row.node_group_ids] : [])
    setGroupPick('')
    setFormError('')
    setSidebarOpen(true)
  }, [])

  function submitForm() {
    setFormError('')
    if (!form.name.trim()) {
      setFormError(t('plans.nameRequired'))
      return
    }
    if (nodeGroupIds.length === 0) {
      setFormError(t('messages.nodeGroupRequired'))
      return
    }
    if (allowedProtocols.length === 0) {
      setFormError(t('plans.protocolRequired'))
      return
    }
    try {
      buildPlanPayload(form, allowedProtocols, nodeGroupIds)
    } catch {
      setFormError(t('plans.trafficQuotaInvalid'))
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
          <span className="data-table-cell" title={row.description?.trim() || undefined}>
            {row.name}
          </span>
        ),
      },
      {
        key: 'node_groups',
        title: t('plans.serverGroups'),
        width: 140,
        render: (row) => (
          <span className="data-table-cell" title={(row.node_group_names ?? []).join(', ')}>
            {(row.node_group_names ?? []).length
              ? row.node_group_names.join('、')
              : '—'}
          </span>
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
        width: 88,
        render: (row) => (
          <span className="data-table-cell">{t(`enums.billingPeriod.${row.traffic_period}`)}</span>
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
        title={t('pages.plans')}
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
        initialWidth={600}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saving} onClick={submitForm}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          {formError ? <p className="sidebar-form-error" role="alert">{formError}</p> : null}
          <FormField label={t('forms.planName')} required>
            <Input
              value={form.name}
              onChange={(event) => updateField('name', event.target.value)}
              maxLength={100}
              placeholder={t('plans.namePlaceholder')}
              required
            />
          </FormField>
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
              value={form.traffic_period}
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
