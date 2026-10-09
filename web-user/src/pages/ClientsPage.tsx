import { useCallback, useEffect, useMemo, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Download, Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage } from '@/api/page'
import { Button } from '@/components/Button'
import { ClientSystemCell } from '@/components/ClientSystemCell'
import {
  ClientTrafficInCell,
  ClientTrafficOutCell,
  ClientTrafficSpeedCell,
  type ClientTrafficMetrics,
} from '@/components/ClientTrafficCells'
import { buildEntityListLeadingColumns } from '@/components/EntityListLeadingCells'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { InstallDialog } from '@/components/InstallDialog'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { useBrandingStore } from '@/stores/branding'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime } from '@/utils/formatDateTime'

type Client = {
  id: string
  user_id: string
  user_email?: string | null
  name: string
  enabled: boolean
  online: boolean
  version?: string | null
  os?: string | null
  arch?: string | null
  public_ip?: string
  region?: string
  tunnel_count?: number
  last_seen_at?: string | null
  traffic_metrics?: ClientTrafficMetrics | null
}

function cellText(value: string | null | undefined) {
  return value?.trim() ? value : '—'
}

export default function ClientsPage({ audience }: { audience: 'admin' | 'user' }) {
  const { t, i18n } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [installTarget, setInstallTarget] = useState<{ id: string; name: string; token?: string } | null>(null)
  const branding = useBrandingStore((state) => state.branding)
  const loadBranding = useBrandingStore((state) => state.load)
  useEffect(() => {
    void loadBranding()
  }, [loadBranding])
  // client 连用户端：用户面板直接用当前地址；管理面板取系统设置里的站点地址。
  const defaultBoard = audience === 'user' ? window.location.origin : (branding?.site_url?.trim() || '')
  const endpoint = audience === 'admin' ? '/v1/admin/clients' : '/v1/clients'
  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<Client>(endpoint, params),
    [endpoint],
  )
  const create = useMutation({
    mutationFn: async () => {
      const path = audience === 'admin' ? '/v1/admin/clients' : '/v1/clients'
      return (await apiClient.post(path, { name })).data as { id: string; name: string; token: string }
    },
    onSuccess: async (result) => {
      setSidebarOpen(false)
      setName('')
      setInstallTarget({ id: result.id, name: result.name, token: result.token })
      await queryClient.invalidateQueries({ queryKey: ['clients'] })
      toast.success(t('common.createSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const clientBase = audience === 'admin' ? '/v1/admin/clients' : '/v1/clients'

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      await apiClient.patch(`${clientBase}/${id}/enabled`, { enabled })
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['clients'] })
      toast.success(t('common.operationSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`${clientBase}/${id}`)
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['clients'] })
      toast.success(t('common.deleteSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editingId) return
      await apiClient.put(`${clientBase}/${editingId}`, { name: name.trim() })
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      setName('')
      await queryClient.invalidateQueries({ queryKey: ['clients'] })
      toast.success(t('common.saveSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const openEdit = useCallback((row: Client) => {
    setEditingId(row.id)
    setName(row.name)
    setSidebarOpen(true)
  }, [])

  const columns: Column<Client>[] = useMemo(() => [
    ...buildEntityListLeadingColumns<Client>({
      t,
      locale: i18n.language,
      nameFieldKey: 'client_name',
      ipColumnKey: 'public_ip',
      pick: (row) => ({
        id: row.id,
        online: row.online,
        region: row.region ?? '',
        ip: row.public_ip,
        name: row.name,
      }),
    }),
    ...(audience === 'admin'
      ? [{
          key: 'user_email' as const,
          title: translateField(t, 'user_email'),
          width: 200,
          render: (row: Client) => (
            <span className="data-table-cell">{cellText(row.user_email)}</span>
          ),
        }]
      : []),
    {
      key: 'system',
      title: translateField(t, 'system'),
      width: 200,
      render: (row) => <ClientSystemCell os={row.os} arch={row.arch} />,
    },
    {
      key: 'traffic_in',
      title: translateField(t, 'traffic_in'),
      width: 100,
      render: (row) => <ClientTrafficInCell metrics={row.traffic_metrics} />,
    },
    {
      key: 'traffic_out',
      title: translateField(t, 'traffic_out'),
      width: 100,
      render: (row) => <ClientTrafficOutCell metrics={row.traffic_metrics} />,
    },
    {
      key: 'traffic_speed',
      title: translateField(t, 'bandwidth'),
      width: 140,
      render: (row) => <ClientTrafficSpeedCell metrics={row.traffic_metrics} />,
    },
    {
      key: 'tunnel_count',
      title: translateField(t, 'tunnel_count'),
      width: 72,
      render: (row) => <span className="data-table-cell">{row.tunnel_count ?? 0}</span>,
    },
    {
      key: 'last_seen_at',
      title: translateField(t, 'last_seen_at'),
      width: 168,
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
          <button type="button" className="data-table-link-btn" onClick={() => openEdit(row)}>
            {t('common.edit')}
          </button>
          <button type="button" className="data-table-link-btn" onClick={() => setInstallTarget({ id: row.id, name: row.name })}>
            <Download size={12} style={{ verticalAlign: '-1px', marginRight: 2 }} />
            {t('install.action')}
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
              if (window.confirm(t('client.deleteConfirm', { name: row.name }))) {
                remove.mutate(row.id)
              }
            }}
          >
            {t('common.delete')}
          </button>
        </div>
      ),
    },
  ], [audience, i18n.language, openEdit, remove.mutate, setEnabled.mutate, t])

  function openCreate() {
    setEditingId(null)
    setName('')
    setSidebarOpen(true)
  }

  function submitSidebar() {
    if (!name.trim()) {
      toast.error(t('messages.clientNameRequired'))
      return
    }
    if (editingId) {
      update.mutate()
    } else {
      create.mutate()
    }
  }

  const sidebarSaving = editingId ? update.isPending : create.isPending

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('forms.createClient')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['clients', audience] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        listLayoutId="clients"
        columns={columns}
        queryKey={['clients', audience]}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
        refetchInterval={audience === 'admin' ? 2000 : false}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('forms.editClient') : t('forms.createClient')}
        onClose={() => {
          setSidebarOpen(false)
          setEditingId(null)
        }}
        initialWidth={480}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={sidebarSaving} onClick={submitSidebar}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          <FormField label={translateField(t, 'client_name')} required>
            <Input value={name} onChange={(event) => setName(event.target.value)} maxLength={100} required />
          </FormField>
        </FormStack>
      </GenericSidebar>
      <InstallDialog
        open={installTarget !== null}
        onClose={() => setInstallTarget(null)}
        role="client"
        name={installTarget?.name ?? ''}
        tokenEndpoint={`${clientBase}/${installTarget?.id ?? ''}/token`}
        defaultBoard={defaultBoard}
        initialToken={installTarget?.token}
      />
    </section>
  )
}
