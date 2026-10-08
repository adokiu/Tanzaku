import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { buildEntityListLeadingColumns } from '@/components/EntityListLeadingCells'
import { CountryRegionSelect } from '@/components/CountryRegionSelect'
import { NodeGroupTagInput } from '@/components/NodeGroupTagInput/NodeGroupTagInput'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { Modal } from '@/components/Modal/Modal'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime } from '@/utils/formatDateTime'
import {
  NodeConnectionsCell,
  NodeCpuCell,
  NodeMemoryCell,
  NodeNetCell,
  type HostMetrics,
} from '@/components/NodeHostMetricsCells'

type NodeGroup = { id: string; name: string; enabled: boolean }

type NodeListItem = {
  id: string
  name: string
  region: string
  public_host: string
  enabled: boolean
  online: boolean
  tunnel_count: number
  last_seen_at: string | null
  host_metrics?: HostMetrics | null
}

type DataplaneCatalog = {
  protocols: string[]
  carriers: { kind: string; socket: string }[]
}

type NodeDetail = {
  id: string
  name: string
  region: string
  public_host: string
  bind_addr: string
  enabled: boolean
  online: boolean
  protocols: string[]
  carrier_ports: Record<string, { port: number; enabled: boolean }>
  tcp_port_ranges: number[][]
  udp_port_ranges: number[][]
  port_exclude: number[][]
  http_shared_port: number
  https_shared_port: number
  node_group_names: string[]
}

type CarrierForm = { port: string; enabled: boolean }

type NodeForm = {
  name: string
  region: string
  public_host: string
  bind_addr: string
  node_group_names: string[]
  protocols: string[]
  carriers: Record<string, CarrierForm>
  tcp_port_ranges: string
  udp_port_ranges: string
  port_exclude: string
  http_shared_port: string
  https_shared_port: string
}

function emptyCarriers(catalog: DataplaneCatalog | undefined, enabled: boolean): Record<string, CarrierForm> {
  const carriers: Record<string, CarrierForm> = {}
  for (const carrier of catalog?.carriers ?? []) {
    carriers[carrier.kind] = { port: '7000', enabled }
  }
  return carriers
}

const initialForm: NodeForm = {
  name: '', region: '', public_host: '', bind_addr: '0.0.0.0', node_group_names: [],
  protocols: [], carriers: {},
  tcp_port_ranges: '20000-29999', udp_port_ranges: '20000-29999', port_exclude: '',
  http_shared_port: '80', https_shared_port: '443',
}

function rangesToString(ranges: number[][]): string {
  if (!ranges.length) return ''
  return ranges.map(([start, end]) => `${start}-${end}`).join(',')
}

function detailToForm(detail: NodeDetail, catalog: DataplaneCatalog | undefined): NodeForm {
  const carriers = emptyCarriers(catalog, false)
  for (const [kind, value] of Object.entries(detail.carrier_ports ?? {})) {
    carriers[kind] = {
      port: String(value?.port ?? 7000),
      enabled: value?.enabled !== false,
    }
  }
  return {
    name: detail.name,
    region: detail.region,
    public_host: detail.public_host,
    bind_addr: detail.bind_addr || '0.0.0.0',
    node_group_names: detail.node_group_names ?? [],
    protocols: (detail.protocols ?? []).filter((protocol) =>
      catalog ? catalog.protocols.includes(protocol) : true,
    ),
    carriers,
    tcp_port_ranges: rangesToString(detail.tcp_port_ranges),
    udp_port_ranges: rangesToString(detail.udp_port_ranges),
    port_exclude: rangesToString(detail.port_exclude),
    http_shared_port: String(detail.http_shared_port),
    https_shared_port: String(detail.https_shared_port),
  }
}

function buildPayload(form: NodeForm, catalog: DataplaneCatalog | undefined) {
  return {
    name: form.name,
    region: form.region.trim().toUpperCase(),
    public_host: form.public_host,
    bind_addr: form.bind_addr || null,
    node_group_names: form.node_group_names,
    protocols: form.protocols,
    carriers: (catalog?.carriers ?? []).map((carrier) => ({
      kind: carrier.kind,
      port: Number(form.carriers[carrier.kind]?.port || 7000),
      enabled: Boolean(form.carriers[carrier.kind]?.enabled),
    })),
    http_shared_port: Number(form.http_shared_port),
    https_shared_port: Number(form.https_shared_port),
    tcp_port_ranges: form.tcp_port_ranges,
    udp_port_ranges: form.udp_port_ranges,
    port_exclude: form.port_exclude.trim() || null,
  }
}


export default function NodesPage() {
  const { t, i18n } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState(initialForm)
  const [formError, setFormError] = useState('')
  const [issuedToken, setIssuedToken] = useState('')
  const [tokenModalOpen, setTokenModalOpen] = useState(false)
  const [groupSearch, setGroupSearch] = useState('')

  const catalog = useQuery({
    queryKey: ['admin-dataplane'],
    queryFn: async () => (await apiClient.get('/v1/admin/dataplane')).data as DataplaneCatalog,
  })

  const groups = useQuery({
    queryKey: ['admin-node-groups', groupSearch.trim()],
    queryFn: () => getPageItems<NodeGroup>('/v1/admin/node-groups', groupSearch.trim() ? { q: groupSearch.trim() } : undefined),
    enabled: sidebarOpen,
  })

  const groupSuggestions = useMemo(
    () => (groups.data ?? []).map((group) => ({ id: group.id, name: group.name, enabled: group.enabled })),
    [groups.data],
  )

  const fetchNodes = useCallback(
    (params: { page: number; page_size: number }) => getPage<NodeListItem>('/v1/admin/nodes', params),
    [],
  )

  const create = useMutation({
    mutationFn: async () => (await apiClient.post('/v1/admin/nodes', buildPayload(form, catalog.data))).data as { id: string; token: string },
    onSuccess: async (node) => {
      setSidebarOpen(false)
      setEditingId(null)
      setForm(initialForm)
      setFormError('')
      setGroupSearch('')
      setIssuedToken(node.token)
      setTokenModalOpen(true)
      await queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })
      await queryClient.invalidateQueries({ queryKey: ['admin-node-groups'] })
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => apiClient.put(`/v1/admin/nodes/${editingId}`, buildPayload(form, catalog.data)),
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      setForm(initialForm)
      setFormError('')
      setGroupSearch('')
      await queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })
      await queryClient.invalidateQueries({ queryKey: ['admin-node-groups'] })
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      await apiClient.patch(`/v1/admin/nodes/${id}/enabled`, { enabled })
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })
    },
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/nodes/${id}`)
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })
    },
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const columns: Column<NodeListItem>[] = useMemo(() => [
    ...buildEntityListLeadingColumns<NodeListItem>({
      t,
      locale: i18n.language,
      nameFieldKey: 'node_name',
      ipColumnKey: 'public_host',
      pick: (row) => ({
        id: row.id,
        online: row.online,
        region: row.region,
        ip: row.public_host,
        name: row.name,
      }),
    }),
    {
      key: 'cpu',
      title: translateField(t, 'cpu'),
      width: 116,
      render: (row) => <NodeCpuCell metrics={row.host_metrics} />,
    },
    {
      key: 'memory',
      title: translateField(t, 'memory'),
      width: 116,
      render: (row) => <NodeMemoryCell metrics={row.host_metrics} />,
    },
    {
      key: 'bandwidth',
      title: translateField(t, 'bandwidth'),
      width: 140,
      render: (row) => <NodeNetCell metrics={row.host_metrics} />,
    },
    {
      key: 'tunnel_count',
      title: translateField(t, 'tunnel_count'),
      width: 72,
      render: (row) => <span className="data-table-cell">{row.tunnel_count}</span>,
    },
    {
      key: 'connections',
      title: translateField(t, 'connections'),
      width: 72,
      render: (row) => <NodeConnectionsCell metrics={row.host_metrics} />,
    },
    {
      key: 'last_seen_at',
      title: translateField(t, 'last_seen_at'),
      width: 156,
      render: (row) => (
        <span className="data-table-cell">
          {row.last_seen_at ? formatDateTime(row.last_seen_at) : '—'}
        </span>
      ),
    },
    {
      key: 'actions',
      title: translateField(t, 'action'),
      render: (row) => (
        <div className="data-table-actions">
          <button type="button" className="data-table-link-btn" onClick={() => openEdit(row.id)}>
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
              if (window.confirm(t('node.deleteConfirm', { name: row.name }))) {
                remove.mutate(row.id)
              }
            }}
          >
            {t('common.delete')}
          </button>
        </div>
      ),
    },
  ], [i18n.language, remove.mutate, setEnabled.mutate, t])

  function updateField<K extends keyof NodeForm>(key: K, value: NodeForm[K]) {
    setForm((current) => ({ ...current, [key]: value }))
    setFormError('')
  }

  function openCreate() {
    setEditingId(null)
    setForm({
      ...initialForm,
      protocols: catalog.data?.protocols ?? [],
      carriers: emptyCarriers(catalog.data, true),
    })
    setFormError('')
    setGroupSearch('')
    setSidebarOpen(true)
  }

  async function openEdit(id: string) {
    setFormError('')
    setGroupSearch('')
    try {
      const detail = (await apiClient.get(`/v1/admin/nodes/${id}`)).data as NodeDetail
      setEditingId(id)
      setForm(detailToForm(detail, catalog.data))
      setSidebarOpen(true)
    } catch (error) {
      window.alert(translateApiError(t, error))
    }
  }

  function toggleProtocol(kind: string) {
    setForm((current) => ({
      ...current,
      protocols: current.protocols.includes(kind)
        ? current.protocols.filter((item) => item !== kind)
        : [...current.protocols, kind],
    }))
    setFormError('')
  }

  function toggleCarrier(kind: string, enabled: boolean) {
    setForm((current) => ({
      ...current,
      carriers: {
        ...current.carriers,
        [kind]: { port: current.carriers[kind]?.port || '7000', enabled },
      },
    }))
    setFormError('')
  }

  function setCarrierPort(kind: string, port: string) {
    setForm((current) => ({
      ...current,
      carriers: {
        ...current.carriers,
        [kind]: { port, enabled: current.carriers[kind]?.enabled ?? false },
      },
    }))
    setFormError('')
  }

  function submitForm() {
    setFormError('')
    if (!form.region.trim()) {
      setFormError(t('messages.regionRequired'))
      return
    }
    if (form.node_group_names.length === 0) {
      setFormError(t('messages.nodeGroupRequired'))
      return
    }
    if (form.protocols.length === 0) {
      setFormError(t('node.protocolRequired'))
      return
    }
    if (!(catalog.data?.carriers ?? []).some((carrier) => form.carriers[carrier.kind]?.enabled)) {
      setFormError(t('node.carrierRequired'))
      return
    }
    if (editingId) {
      update.mutate()
    } else {
      create.mutate()
    }
  }

  const saving = create.isPending || update.isPending

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        title={t('pages.nodes')}
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('forms.createNode')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        listLayoutId="nodes"
        columns={columns}
        queryKey={['admin-nodes']}
        fetchPage={fetchNodes}
        rowKey={(row) => row.id}
        refetchInterval={2000}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('forms.editNode') : t('forms.createNode')}
        onClose={() => setSidebarOpen(false)}
        initialWidth={560}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saving} onClick={submitForm}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          {formError ? <p className="sidebar-form-error" role="alert">{formError}</p> : null}
          <FormField label={translateField(t, 'node_name')} required>
            <Input value={form.name} onChange={(event) => updateField('name', event.target.value)} maxLength={100} required />
          </FormField>
          <FormField label={translateField(t, 'region')} required>
            <CountryRegionSelect
              value={form.region}
              onChange={(code) => updateField('region', code)}
              placeholder={t('select.region')}
            />
          </FormField>
          <FormField label={translateField(t, 'public_host')} required>
            <Input value={form.public_host} onChange={(event) => updateField('public_host', event.target.value)} maxLength={253} required />
          </FormField>
          <FormField label={t('forms.bindListenAddress')}>
            <Input value={form.bind_addr} onChange={(event) => updateField('bind_addr', event.target.value)} />
          </FormField>
          <FormField label={t('nodeGroup.fieldLabel')} required>
            <NodeGroupTagInput
              value={form.node_group_names}
              onChange={(names) => updateField('node_group_names', names)}
              suggestions={groupSuggestions}
              onDraftChange={setGroupSearch}
            />
          </FormField>
          <FormField label={t('node.protocols')} required hint={t('node.registryHint')}>
            <div className="flex flex-wrap gap-4">
              {(catalog.data?.protocols ?? []).map((protocol) => (
                <label className="flex items-center gap-2 text-sm" key={protocol}>
                  <input type="checkbox" checked={form.protocols.includes(protocol)} onChange={() => toggleProtocol(protocol)} />
                  {t(`enums.protocol.${protocol}`, { defaultValue: protocol.toUpperCase() })}
                </label>
              ))}
            </div>
          </FormField>
          <FormField label={t('node.carriers')} required>
            <div className="grid gap-3">
              {(catalog.data?.carriers ?? []).map((carrier) => {
                const current = form.carriers[carrier.kind] ?? { port: '7000', enabled: false }
                return (
                  <div className="flex items-center gap-3" key={carrier.kind}>
                    <label className="flex min-w-24 items-center gap-2 text-sm">
                      <input type="checkbox" checked={current.enabled} onChange={(event) => toggleCarrier(carrier.kind, event.target.checked)} />
                      {t(`enums.protocol.${carrier.kind}`, { defaultValue: carrier.kind.toUpperCase() })}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      max={65535}
                      value={current.port}
                      disabled={!current.enabled}
                      onChange={(event) => setCarrierPort(carrier.kind, event.target.value)}
                    />
                  </div>
                )
              })}
            </div>
          </FormField>
          <FormField label={translateField(t, 'tcp_port_ranges')} required>
            <Input value={form.tcp_port_ranges} onChange={(event) => updateField('tcp_port_ranges', event.target.value)} required />
          </FormField>
          <FormField label={translateField(t, 'udp_port_ranges')} required>
            <Input value={form.udp_port_ranges} onChange={(event) => updateField('udp_port_ranges', event.target.value)} required />
          </FormField>
          <FormField label={translateField(t, 'port_exclude')}>
            <Input value={form.port_exclude} onChange={(event) => updateField('port_exclude', event.target.value)} />
          </FormField>
          <FormField label={translateField(t, 'http_shared_port')} required>
            <Input type="number" min={1} max={65535} value={form.http_shared_port} onChange={(event) => updateField('http_shared_port', event.target.value)} required />
          </FormField>
          <FormField label={translateField(t, 'https_shared_port')} required>
            <Input type="number" min={1} max={65535} value={form.https_shared_port} onChange={(event) => updateField('https_shared_port', event.target.value)} required />
          </FormField>
        </FormStack>
      </GenericSidebar>
      <Modal open={tokenModalOpen} onClose={() => setTokenModalOpen(false)} title={translateField(t, 'one_time_node_token')} footer={(
        <Button onClick={() => setTokenModalOpen(false)}>{t('common.save')}</Button>
      )}>
        <p className="mb-3 text-sm text-muted-foreground">{t('messages.nodeCreated')}</p>
        <Input readOnly value={issuedToken} className="font-mono text-xs" />
      </Modal>
    </section>
  )
}
