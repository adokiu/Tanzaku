import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw, Shield } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage, getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'
import { PolicyFormFields } from './security/PolicyFormFields'
import { PolicySummaryTags } from './security/PolicySummaryTags'
import {
  EMPTY_FORM,
  formToPolicy,
  parseCidrLines,
  policyToForm,
  withOverlayEnabled,
  type GuardPolicy,
  type NodeSecurity,
  type PolicyForm,
} from './security/policyShared'
import './security/security.css'

export default function NodeSecurityPolicyPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [globalOpen, setGlobalOpen] = useState(false)
  const [nodeOpen, setNodeOpen] = useState(false)
  const [creating, setCreating] = useState(false)
  const [nodeId, setNodeId] = useState('')
  const [nodeName, setNodeName] = useState('')
  const [overlayEnabled, setOverlayEnabled] = useState(true)
  const [globalForm, setGlobalForm] = useState<PolicyForm>(EMPTY_FORM)
  const [nodeForm, setNodeForm] = useState<PolicyForm>(EMPTY_FORM)
  const [formError, setFormError] = useState('')

  const globalQuery = useQuery({
    queryKey: ['admin-security-global'],
    queryFn: async () => (await apiClient.get('/v1/admin/security/global')).data as { guard_policy: GuardPolicy },
  })

  const availableNodes = useQuery({
    queryKey: ['admin-security-available'],
    queryFn: () => getPageItems<NodeSecurity>('/v1/admin/security', { overlay: 'none' }),
  })
  const fetchOverlays = useCallback(
    (params: { page: number; page_size: number }) =>
      getPage<NodeSecurity>('/v1/admin/security', { ...params, overlay: 'only' }),
    [],
  )
  const nodesWithoutOverlay = availableNodes.data ?? []

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-security-global'] })
    await queryClient.invalidateQueries({ queryKey: ['admin-security'] })
    await queryClient.invalidateQueries({ queryKey: ['admin-security-available'] })
  }

  const saveGlobal = useMutation({
    mutationFn: async () =>
      apiClient.put('/v1/admin/security/global', { guard_policy: formToPolicy(globalForm) }),
    onSuccess: async () => {
      setGlobalOpen(false)
      setFormError('')
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const saveNode = useMutation({
    mutationFn: async () =>
      apiClient.put(`/v1/admin/security/${nodeId}`, {
        guard_policy: formToPolicy(nodeForm, { asNodeOverlay: true, overlayEnabled }),
        trusted_proxies: parseCidrLines(nodeForm.trustedProxies),
      }),
    onSuccess: async () => {
      setNodeOpen(false)
      setCreating(false)
      setFormError('')
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const toggleOverlay = useMutation({
    mutationFn: async ({ node, enabled }: { node: NodeSecurity; enabled: boolean }) =>
      apiClient.put(`/v1/admin/security/${node.id}`, {
        guard_policy: withOverlayEnabled(node.guard_policy, enabled),
        trusted_proxies: node.trusted_proxies,
      }),
    onSuccess: async () => {
      await invalidate()
    },
  })

  const deleteOverlay = useMutation({
    mutationFn: async (node: NodeSecurity) =>
      apiClient.put(`/v1/admin/security/${node.id}`, {
        guard_policy: {},
        trusted_proxies: node.trusted_proxies,
      }),
    onSuccess: async () => {
      await invalidate()
    },
  })

  function openGlobal() {
    setGlobalForm(policyToForm(globalQuery.data?.guard_policy ?? {}))
    setFormError('')
    setGlobalOpen(true)
  }

  function openCreate() {
    const first = nodesWithoutOverlay[0]
    setCreating(true)
    setNodeId(first?.id ?? '')
    setNodeName(first?.name ?? '')
    setOverlayEnabled(true)
    setNodeForm(policyToForm(globalQuery.data?.guard_policy ?? {}))
    setFormError('')
    setNodeOpen(true)
  }

  function openNode(node: NodeSecurity) {
    setCreating(false)
    setNodeId(node.id)
    setNodeName(node.name)
    setOverlayEnabled(node.overlay_enabled)
    setNodeForm(policyToForm(node.effective_policy ?? node.guard_policy, node.trusted_proxies))
    setFormError('')
    setNodeOpen(true)
  }

  const nodeColumns: Column<NodeSecurity>[] = useMemo(
    () => [
      {
        key: 'name',
        title: translateField(t, 'node_name'),
        render: (row) => <span className="data-table-cell">{row.name}</span>,
      },
      {
        key: 'status',
        title: t('security.overlayStatus'),
        width: 96,
        render: (row) => (
          <span className={`data-table-tag ${row.overlay_enabled ? 'data-table-tag--online' : 'data-table-tag--disabled'}`}>
            {row.overlay_enabled ? t('common.enabled') : t('common.disabled')}
          </span>
        ),
      },
      {
        key: 'effective',
        title: t('security.effectiveSummary'),
        render: (row) => <PolicySummaryTags policy={row.effective_policy} />,
      },
      {
        key: 'action',
        title: translateField(t, 'action'),
        width: 220,
        render: (row) => (
          <div className="policy-row-actions">
            <button type="button" className="data-table-link-btn" onClick={() => openNode(row)}>
              {t('common.edit')}
            </button>
            <button
              type="button"
              className="data-table-link-btn"
              disabled={toggleOverlay.isPending}
              onClick={() => toggleOverlay.mutate({ node: row, enabled: !row.overlay_enabled })}
            >
              {row.overlay_enabled ? t('common.disable') : t('common.enable')}
            </button>
            <button
              type="button"
              className="data-table-link-btn data-table-link-btn--danger"
              disabled={deleteOverlay.isPending}
              onClick={() => {
                if (window.confirm(t('security.confirmDeleteOverlay'))) {
                  deleteOverlay.mutate(row)
                }
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ],
    [t, toggleOverlay.isPending, deleteOverlay.isPending],
  )

  const serverOptions = useMemo(
    () =>
      nodesWithoutOverlay.map((node) => ({
        value: node.id,
        label: node.name,
        searchText: node.name,
      })),
    [nodesWithoutOverlay],
  )

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.securityPolicy')}
        actions={(
          <>
            <Button variant="secondary" size="sm" onClick={openGlobal}>
              <Shield size={16} />
              {t('security.editGlobal')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={openCreate}
              disabled={nodesWithoutOverlay.length === 0}
            >
              <Plus size={16} />
              {t('security.addServerPolicy')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void invalidate()}
            >
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />

      <div className="page-card mb-4 p-5">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2 className="text-sm font-medium">{t('security.globalTitle')}</h2>
          <button type="button" className="data-table-link-btn" onClick={openGlobal}>
            {t('common.edit')}
          </button>
        </div>
        <PolicySummaryTags policy={globalQuery.data?.guard_policy ?? {}} />
      </div>

      <h2 className="mb-2 text-sm font-medium">{t('security.nodesTitle')}</h2>
      <p className="mb-3 text-xs text-muted-foreground">{t('security.nodesListHint')}</p>
      <EntityList
        columns={nodeColumns}
        queryKey={['admin-security']}
        fetchPage={fetchOverlays}
        rowKey={(row) => row.id}
        refetchInterval={5000}
        emptyText={t('security.noServerOverlay')}
        tableClassName=""
      />

      <GenericSidebar
        open={globalOpen}
        title={t('security.editGlobal')}
        onClose={() => setGlobalOpen(false)}
        initialWidth={560}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setGlobalOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saveGlobal.isPending} onClick={() => saveGlobal.mutate()}>{t('common.save')}</Button>
          </>
        )}
      >
        {formError ? <p className="sidebar-form-error mb-3" role="alert">{formError}</p> : null}
        <PolicyFormFields form={globalForm} setForm={setGlobalForm} />
      </GenericSidebar>

      <GenericSidebar
        open={nodeOpen}
        title={creating ? t('security.addServerPolicy') : `${t('forms.saveSecurity')} — ${nodeName}`}
        onClose={() => {
          setNodeOpen(false)
          setCreating(false)
        }}
        initialWidth={560}
        footer={(
          <>
            <Button
              variant="ghost"
              onClick={() => {
                setNodeOpen(false)
                setCreating(false)
              }}
            >
              {t('common.cancel')}
            </Button>
            <Button
              loading={saveNode.isPending}
              disabled={!nodeId}
              onClick={() => saveNode.mutate()}
            >
              {t('common.save')}
            </Button>
          </>
        )}
      >
        {formError ? <p className="sidebar-form-error mb-3" role="alert">{formError}</p> : null}
        {creating ? (
          <FormField label={t('security.selectServer')} required>
            <Select
              value={nodeId}
              options={serverOptions}
              placeholder={t('security.selectServerPlaceholder')}
              searchable
              emptyText={t('security.noServerAvailable')}
              onChange={(value) => {
                const id = String(value)
                setNodeId(id)
                setNodeName(nodesWithoutOverlay.find((node) => node.id === id)?.name ?? '')
              }}
            />
          </FormField>
        ) : null}
        <div className={creating ? 'mt-4' : undefined}>
          <PolicyFormFields form={nodeForm} setForm={setNodeForm} showTrustedProxies />
        </div>
      </GenericSidebar>
    </section>
  )
}
