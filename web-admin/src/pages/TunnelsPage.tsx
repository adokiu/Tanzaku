import { useCallback, useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityIdCell } from '@/components/EntityListLeadingCells'
import { EntityListPage, type PaginatedListResponse } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { Select } from '@/components/Select/Select'
import {
  TunnelCarrierTag,
  TunnelProtocolTag,
  TunnelStatusCell,
  TunnelTargetCell,
  TunnelTrafficSpeedCell,
  TunnelTrafficTotalCell,
} from '@/components/TunnelListCells'
import type { ClientTrafficMetrics } from '@/components/ClientTrafficCells'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'

type NodeOption = { id: string; name: string }

type TunnelRow = {
  id: string
  user_id: string
  user_email?: string | null
  client_id: string
  client_name?: string | null
  node_id: string
  name: string
  carrier: string
  protocol: string
  remote_port: number | null
  target_host: string | null
  target_port: number | null
  target_url: string | null
  https_enabled?: boolean
  cert_id?: string | null
  domains?: string[]
  http_access?: string | null
  host_rewrite?: string | null
  backend_tls_insecure?: boolean
  speed_limit_mbps?: number
  status: string
  enabled: boolean
  traffic_metrics?: ClientTrafficMetrics | null
}

type ClientOption = { id: string; user_id?: string; name: string; user_email?: string | null }

type CertificateOption = {
  id: string
  owner_user_id?: string | null
  domains: string[]
  not_after: string
}

type DataplaneCatalog = {
  protocols: string[]
  carriers: { kind: string; socket: string }[]
}

function parseDomainList(raw: string): string[] {
  return raw.split(/[\s,，]+/).map((item) => item.trim()).filter(Boolean)
}

function isIpHost(value: string) {
  const host = value.trim().replace(/^\[|\]$/g, '')
  if (host.includes(':')) return /^[0-9a-f:]+$/i.test(host) && host.includes(':')
  const parts = host.split('.')
  return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}

function domainMatchScore(certDomains: string[], tunnelDomains: string[]): number {
  let score = 0
  for (const domain of tunnelDomains) {
    const normalized = domain.trim().toLowerCase().replace(/\.$/, '')
    let best = 0
    for (const raw of certDomains) {
      const candidate = raw.trim().toLowerCase()
      if (candidate === normalized) {
        best = Math.max(best, 3)
      } else if (candidate.startsWith('*.')) {
        const suffix = candidate.slice(2)
        const prefix = `.${suffix}`
        if (normalized.endsWith(prefix)) {
          const label = normalized.slice(0, normalized.length - prefix.length)
          if (label && !label.includes('.')) best = Math.max(best, 2)
        }
      }
    }
    score += best
  }
  return score
}

export default function TunnelsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const nodesQuery = useQuery({
    queryKey: ['admin-nodes-options'],
    queryFn: () => getPageItems<NodeOption>('/v1/admin/nodes'),
  })
  const [nodeId, setNodeId] = useState('')
  useEffect(() => {
    if (!nodeId && nodesQuery.data?.length) {
      setNodeId(nodesQuery.data[0].id)
    }
  }, [nodeId, nodesQuery.data])

  const catalogQuery = useQuery({
    queryKey: ['admin-dataplane'],
    queryFn: async () => (await apiClient.get('/v1/admin/dataplane')).data as DataplaneCatalog,
  })
  const nodeDetailQuery = useQuery({
    queryKey: ['admin-node', nodeId],
    queryFn: async () => (await apiClient.get(`/v1/admin/nodes/${nodeId}`)).data as {
      protocols: string[]
      carrier_ports: Record<string, { port: number; enabled: boolean }>
    },
    enabled: Boolean(nodeId),
  })
  const carrierOptions = (catalogQuery.data?.carriers ?? []).filter((item) => nodeDetailQuery.data?.carrier_ports?.[item.kind]?.enabled)
  const protocolOptions = (catalogQuery.data?.protocols ?? []).filter((item) => (nodeDetailQuery.data?.protocols ?? []).includes(item))

  const clientsQuery = useQuery({
    queryKey: ['admin-clients-options'],
    queryFn: () => getPageItems<ClientOption>('/v1/admin/clients'),
  })

  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editing, setEditing] = useState<TunnelRow | null>(null)
  const [formError, setFormError] = useState('')
  const [name, setName] = useState('')
  const [clientId, setClientId] = useState('')
  const [carrier, setCarrier] = useState('tcp')
  const [protocol, setProtocol] = useState('tcp')
  const [remotePort, setRemotePort] = useState('')
  const [targetHost, setTargetHost] = useState('')
  const [targetPort, setTargetPort] = useState('')
  const [targetUrl, setTargetUrl] = useState('')
  const [httpAccess, setHttpAccess] = useState<'shared' | 'dedicated'>('shared')
  const [domainsText, setDomainsText] = useState('')
  const [httpsEnabled, setHttpsEnabled] = useState(false)
  const [certId, setCertId] = useState('')
  const [hostRewrite, setHostRewrite] = useState('$http_host')
  const [backendTlsInsecure, setBackendTlsInsecure] = useState(false)
  const [speedLimit, setSpeedLimit] = useState('')
  const [formNodeId, setFormNodeId] = useState('')
  const [showAllCerts, setShowAllCerts] = useState(false)

  const isHttp = protocol === 'http'
  const targetIsHttps = isHttp && targetUrl.trim().toLowerCase().startsWith('https://')
  const needsPublicPort = !isHttp || httpAccess === 'dedicated'

  const certificatesQuery = useQuery({
    queryKey: ['admin-certificates-options'],
    queryFn: () => getPageItems<CertificateOption>('/v1/admin/certificates'),
    enabled: sidebarOpen && httpsEnabled,
  })

  const nodeOptions = useMemo(
    () => (nodesQuery.data ?? []).map((node) => ({ label: node.name, value: node.id })),
    [nodesQuery.data],
  )

  const clientOptions = useMemo(
    () =>
      (clientsQuery.data ?? []).map((client) => ({
        label: client.user_email ? `${client.name} (${client.user_email})` : client.name,
        value: client.id,
      })),
    [clientsQuery.data],
  )

  const fetchPage = useCallback(
    async (params: { page: number; page_size: number }) => {
      const response = await apiClient.get('/v1/admin/tunnels', {
        params: { node_id: nodeId, page: params.page, page_size: params.page_size },
      })
      return response.data as PaginatedListResponse<TunnelRow>
    },
    [nodeId],
  )

  const invalidate = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-tunnels', nodeId] })
  }, [nodeId, queryClient])

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      await apiClient.patch(`/v1/admin/tunnels/${id}/enabled`, { enabled })
    },
    onSuccess: invalidate,
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/tunnels/${id}`)
    },
    onSuccess: invalidate,
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const create = useMutation({
    mutationFn: async () => {
      const domains = parseDomainList(domainsText)
      await apiClient.post('/v1/admin/tunnels', {
        node_id: nodeId,
        client_id: clientId,
        name: name.trim(),
        carrier,
        protocol,
        remote_port:
          needsPublicPort && remotePort.trim() ? Number(remotePort) : undefined,
        target_host: isHttp ? undefined : targetHost.trim() || undefined,
        target_port: isHttp ? undefined : targetPort.trim() ? Number(targetPort) : undefined,
        target_url: isHttp ? targetUrl.trim() : undefined,
        http_access: isHttp ? httpAccess : undefined,
        domains: isHttp ? domains : undefined,
        https_enabled: isHttp ? httpsEnabled : undefined,
        cert_id: isHttp && httpsEnabled && certId ? certId : undefined,
        host_rewrite: isHttp ? hostRewrite.trim() || '$http_host' : undefined,
        backend_tls_insecure:
          isHttp && targetIsHttps ? backendTlsInsecure : undefined,
      })
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      resetForm()
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editing) return
      const httpTunnel = editing.protocol === 'http'
      await apiClient.put(`/v1/admin/tunnels/${editing.id}`, {
        name: name.trim(),
        client_id: clientId,
        node_id: formNodeId || editing.node_id,
        remote_port: !httpTunnel || httpAccess === 'dedicated'
          ? (remotePort.trim() ? Number(remotePort) : null)
          : null,
        target_host: httpTunnel ? undefined : targetHost.trim() || undefined,
        target_port: httpTunnel ? undefined : targetPort.trim() ? Number(targetPort) : undefined,
        target_url: httpTunnel ? targetUrl.trim() : undefined,
        http_access: httpTunnel ? httpAccess : undefined,
        domains: httpTunnel ? parseDomainList(domainsText) : undefined,
        https_enabled: httpTunnel ? httpsEnabled : undefined,
        cert_id: httpTunnel && httpsEnabled ? certId : undefined,
        host_rewrite: httpTunnel ? hostRewrite.trim() || '$http_host' : undefined,
        backend_tls_insecure: httpTunnel && targetIsHttps ? backendTlsInsecure : undefined,
        speed_limit_mbps: speedLimit.trim() ? Number(speedLimit) : undefined,
      })
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditing(null)
      resetForm()
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  function resetForm() {
    setName('')
    setClientId('')
    setCarrier(carrierOptions[0]?.kind ?? '')
    setProtocol(protocolOptions[0] ?? '')
    setRemotePort('')
    setTargetHost('')
    setTargetPort('')
    setTargetUrl('')
    setHttpAccess('shared')
    setDomainsText('')
    setHttpsEnabled(false)
    setCertId('')
    setHostRewrite('$http_host')
    setBackendTlsInsecure(false)
    setSpeedLimit('')
    setFormNodeId('')
    setShowAllCerts(false)
    setFormError('')
  }

  function openCreate() {
    setEditing(null)
    resetForm()
    setSidebarOpen(true)
  }

  function openEdit(row: TunnelRow) {
    setEditing(row)
    setName(row.name)
    setClientId(row.client_id)
    setCarrier(row.carrier)
    setProtocol(row.protocol)
    setRemotePort(row.remote_port != null ? String(row.remote_port) : '')
    setTargetHost(row.target_host ?? '')
    setTargetPort(row.target_port != null ? String(row.target_port) : '')
    setTargetUrl(row.target_url ?? '')
    setDomainsText((row.domains ?? []).join('\n'))
    setHttpAccess(row.http_access === 'dedicated' ? 'dedicated' : 'shared')
    setHostRewrite(row.host_rewrite || '$http_host')
    setBackendTlsInsecure(Boolean(row.backend_tls_insecure))
    setHttpsEnabled(Boolean(row.https_enabled))
    setCertId(row.cert_id ?? '')
    setSpeedLimit(row.speed_limit_mbps != null ? String(row.speed_limit_mbps) : '')
    setFormNodeId(row.node_id)
    setShowAllCerts(false)
    setFormError('')
    setSidebarOpen(true)
  }

  function submitSidebar() {
    setFormError('')
    if (!name.trim()) {
      setFormError(t('messages.tunnelNameRequired'))
      return
    }
    if (editing) {
      if (isHttp && httpsEnabled && !certId) {
        setFormError(t('messages.tunnelCertRequired'))
        return
      }
      if (isHttp && !targetUrl.trim()) {
        setFormError(t('messages.tunnelTargetUrlRequired'))
        return
      }
      if (isHttp && (httpAccess === 'shared' || httpsEnabled) && parseDomainList(domainsText).length === 0) {
        setFormError(t('messages.tunnelDomainsRequired'))
        return
      }
      if (isHttp && httpAccess === 'shared' && parseDomainList(domainsText).some(isIpHost)) {
        setFormError(t('messages.tunnelSharedIpForbidden'))
        return
      }
      if (!isHttp && (!targetHost.trim() || !targetPort.trim())) {
        setFormError(t('messages.tunnelTargetRequired'))
        return
      }
      if (!clientId) {
        setFormError(t('messages.clientOwnerRequired'))
        return
      }
      update.mutate()
    } else {
      if (!clientId) {
        setFormError(t('messages.clientOwnerRequired'))
        return
      }
      if (isHttp) {
        if (!targetUrl.trim()) {
          setFormError(t('messages.tunnelTargetUrlRequired'))
          return
        }
        const domains = parseDomainList(domainsText)
        if ((httpAccess === 'shared' || httpsEnabled) && domains.length === 0) {
          setFormError(t('messages.tunnelDomainsRequired'))
          return
        }
        if (httpAccess === 'shared' && domains.some(isIpHost)) {
          setFormError(t('messages.tunnelSharedIpForbidden'))
          return
        }
        if (httpsEnabled && !certId) {
          setFormError(t('messages.tunnelCertRequired'))
          return
        }
      } else if (!targetHost.trim() || !targetPort.trim()) {
        setFormError(t('messages.tunnelTargetRequired'))
        return
      }
      create.mutate()
    }
  }

  const certOwnerId = clientOptions.length
    ? clientsQuery.data?.find((client) => client.id === clientId)?.user_id ?? editing?.user_id
    : editing?.user_id
  const rankedCerts = useMemo(() => {
    const now = Date.now()
    const tunnelDomains = parseDomainList(domainsText)
    return (certificatesQuery.data ?? [])
      .filter((cert) => {
        if (certOwnerId && cert.owner_user_id && cert.owner_user_id !== certOwnerId) return false
        const expires = Date.parse(cert.not_after)
        return Number.isNaN(expires) || expires > now
      })
      .map((cert) => ({
        cert,
        score: domainMatchScore(cert.domains, tunnelDomains),
      }))
      .sort((left, right) => right.score - left.score || left.cert.domains.join(',').localeCompare(right.cert.domains.join(',')))
  }, [certOwnerId, certificatesQuery.data, domainsText])
  const certOptions = useMemo(() => {
    const visible = showAllCerts ? rankedCerts : rankedCerts.filter((item) => item.score > 0 || item.cert.id === certId)
    return visible.map((item) => ({
      label: `${item.cert.domains.join(', ')} · ${item.cert.not_after}`,
      value: item.cert.id,
    }))
  }, [certId, rankedCerts, showAllCerts])
  const hasHiddenCerts = rankedCerts.some((item) => item.score === 0 && item.cert.id !== certId)

  const columns: Column<TunnelRow>[] = useMemo(
    () => [
      {
        key: 'id',
        title: t('fields.id'),
        width: 80,
        fixedWidth: true,
        render: (row) => <EntityIdCell id={row.id} />,
      },
      {
        key: 'user_id',
        title: translateField(t, 'user_id'),
        width: 96,
        fixedWidth: true,
        render: (row) => (
          <EntityIdCell id={row.user_id} title={row.user_email?.trim() || undefined} />
        ),
      },
      {
        key: 'client_id',
        title: translateField(t, 'client_id'),
        width: 96,
        render: (row) => <EntityIdCell id={row.client_id} />,
      },
      {
        key: 'status',
        title: translateField(t, 'status'),
        width: 88,
        fixedWidth: true,
        render: (row) => <TunnelStatusCell status={row.status} enabled={row.enabled} />,
      },
      {
        key: 'protocol',
        title: translateField(t, 'protocol'),
        width: 72,
        render: (row) => <TunnelProtocolTag protocol={row.protocol} />,
      },
      {
        key: 'carrier',
        title: translateField(t, 'carrier'),
        width: 72,
        render: (row) => <TunnelCarrierTag carrier={row.carrier} />,
      },
      {
        key: 'remote_port',
        title: translateField(t, 'remote_port'),
        width: 88,
        render: (row) => (
          <span className="data-table-cell">{row.remote_port ?? '—'}</span>
        ),
      },
      {
        key: 'target',
        title: t('tunnels.targetEndpoint'),
        width: 160,
        render: (row) => (
          <TunnelTargetCell
            targetHost={row.target_host}
            targetPort={row.target_port}
            targetUrl={row.target_url}
          />
        ),
      },
      {
        key: 'traffic_speed',
        title: translateField(t, 'bandwidth'),
        width: 140,
        render: (row) => <TunnelTrafficSpeedCell metrics={row.traffic_metrics} />,
      },
      {
        key: 'traffic_total',
        title: t('tunnels.trafficTotal'),
        width: 100,
        render: (row) => <TunnelTrafficTotalCell metrics={row.traffic_metrics} />,
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
                if (window.confirm(t('tunnels.deleteConfirm', { name: row.name }))) {
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
    [remove.mutate, setEnabled.mutate, t],
  )

  const sidebarSaving = editing ? update.isPending : create.isPending

  return (
    <>
      <EntityListPage
        title={t('pages.tunnels')}
        listLayoutId="tunnels"
        columns={columns}
        queryKey={['admin-tunnels', nodeId]}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
        enabled={Boolean(nodeId)}
        resetPageWhen={nodeId}
        refetchInterval={2000}
        actions={(
          <>
            <Button size="sm" onClick={openCreate} disabled={!nodeId}>
              <Plus size={16} />
              {t('forms.createTunnel')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => invalidate()}
              disabled={!nodeId}
            >
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
        toolbar={(
          <div className="entity-list-toolbar__row">
            <FormField label={t('tunnels.filterByNode')}>
              <Select
                value={nodeId}
                options={nodeOptions}
                onChange={(value) => setNodeId(String(value))}
                placeholder={t('common.select')}
              />
            </FormField>
          </div>
        )}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editing ? t('forms.editTunnel') : t('forms.createTunnel')}
        onClose={() => {
          setSidebarOpen(false)
          setEditing(null)
        }}
        initialWidth={560}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>
              {t('common.cancel')}
            </Button>
            <Button loading={sidebarSaving} onClick={submitSidebar}>
              {t('common.save')}
            </Button>
          </>
        )}
      >
        <FormStack>
          {formError ? <p className="sidebar-form-error" role="alert">{formError}</p> : null}
          {editing ? (
            <FormField label={t('tunnels.filterByNode')} required>
              <Select
                value={formNodeId}
                options={nodeOptions}
                onChange={(value) => setFormNodeId(String(value))}
                placeholder={t('common.select')}
              />
            </FormField>
          ) : null}
          <FormField label={translateField(t, 'client_id')} required>
            <Select
              value={clientId}
              options={clientOptions}
              searchable
              onChange={(value) => setClientId(String(value))}
              placeholder={t('common.select')}
            />
          </FormField>
          <FormField label={translateField(t, 'tunnel_name')} required>
            <Input value={name} onChange={(event) => setName(event.target.value)} maxLength={100} />
          </FormField>
          <FormField label={translateField(t, 'carrier')} required>
            <Select
              value={carrier}
              disabled={Boolean(editing)}
              options={(editing ? [{ kind: carrier }] : carrierOptions).map((item) => ({
                label: t(`enums.protocol.${item.kind}`, { defaultValue: item.kind.toUpperCase() }),
                value: item.kind,
              }))}
              onChange={(value) => setCarrier(String(value))}
            />
          </FormField>
          <FormField label={translateField(t, 'protocol')} required>
            <Select
              value={protocol}
              disabled={Boolean(editing)}
              options={(editing ? [protocol] : protocolOptions).map((item) => ({
                label: t(`enums.protocol.${item}`, { defaultValue: item.toUpperCase() }),
                value: item,
              }))}
              onChange={(value) => setProtocol(String(value))}
            />
          </FormField>
          {needsPublicPort ? (
            <FormField label={translateField(t, 'remote_port')}>
              <Input
                type="number"
                min={1}
                max={65535}
                value={remotePort}
                onChange={(event) => setRemotePort(event.target.value)}
                placeholder={t('tunnels.remotePortOptional')}
              />
            </FormField>
          ) : null}
          {editing ? (
            <FormField label={translateField(t, 'speed_limit_mbps')}>
              <Input
                type="number"
                min={1}
                value={speedLimit}
                onChange={(event) => setSpeedLimit(event.target.value)}
                placeholder={t('tunnels.speedLimitOptional')}
              />
            </FormField>
          ) : null}
          {isHttp ? (
            <>
              <FormField label={translateField(t, 'http_access')} required>
                <Select
                  value={httpAccess}
                  options={[
                    { label: t('tunnels.httpAccessShared'), value: 'shared' },
                    { label: t('tunnels.httpAccessDedicated'), value: 'dedicated' },
                  ]}
                  onChange={(value) => setHttpAccess(value as 'shared' | 'dedicated')}
                />
              </FormField>
              <FormField label={translateField(t, 'target_url')} required>
                <Input
                  type="url"
                  value={targetUrl}
                  onChange={(event) => setTargetUrl(event.target.value)}
                  placeholder="http://127.0.0.1:8080"
                />
              </FormField>
              <FormField label={translateField(t, 'domains')} required={httpAccess === 'shared' || httpsEnabled} hint={[httpAccess === 'shared' ? t('tunnels.sharedDomainHint') : t('tunnels.dedicatedDomainHint'), httpsEnabled ? t('tunnels.httpsDomainHint') : ''].filter(Boolean).join(' ')}>
                <textarea
                  className="apple-input w-full min-h-[80px]"
                  value={domainsText}
                  onChange={(event) => {
                    setDomainsText(event.target.value)
                    setShowAllCerts(false)
                  }}
                  placeholder="service.example.com"
                />
              </FormField>
              <FormField label={t('tunnels.hostRewrite')}>
                <Input
                  value={hostRewrite}
                  onChange={(event) => setHostRewrite(event.target.value)}
                  placeholder="$http_host"
                />
              </FormField>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={httpsEnabled}
                  onChange={(event) => {
                    setHttpsEnabled(event.target.checked)
                    if (!event.target.checked) setCertId('')
                  }}
                />
                {t('tunnels.httpsEnabled')}
              </label>
              {httpsEnabled ? (
                <FormField label={t('tunnels.httpsCert')} required>
                  <Select
                    value={certId}
                    options={certOptions}
                    searchable
                    onChange={(value) => setCertId(String(value))}
                    placeholder={t('common.select')}
                  />
                  {hasHiddenCerts ? (
                    <button
                      type="button"
                      className="data-table-link-btn mt-2"
                      onClick={() => setShowAllCerts((current) => !current)}
                    >
                      {showAllCerts ? t('tunnels.showMatchedCerts') : t('tunnels.showMoreCerts')}
                    </button>
                  ) : null}
                </FormField>
              ) : null}
              {targetIsHttps ? (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={backendTlsInsecure}
                    onChange={(event) => setBackendTlsInsecure(event.target.checked)}
                  />
                  {t('tunnels.backendTlsInsecure')}
                </label>
              ) : null}
            </>
          ) : (
            <>
              <FormField label={t('tunnels.targetHost')} required>
                <Input value={targetHost} onChange={(event) => setTargetHost(event.target.value)} />
              </FormField>
              <FormField label={t('tunnels.targetPort')} required>
                <Input
                  type="number"
                  min={1}
                  max={65535}
                  value={targetPort}
                  onChange={(event) => setTargetPort(event.target.value)}
                />
              </FormField>
            </>
          )}
        </FormStack>
      </GenericSidebar>
    </>
  )
}
