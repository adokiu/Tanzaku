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
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'
import { formatDateTime } from '@/utils/formatDateTime'

type CertificateRow = {
  id: string
  owner_user_id: string | null
  owner_email: string | null
  domains: string[]
  not_before: string
  not_after: string
  source: string
  created_at: string
}

type UserOption = { id: string; email: string }

const SYSTEM_OWNER = ''

function sourceLabel(t: (key: string) => string, source: string) {
  if (source === 'manual') return t('certificates.sourceManual')
  if (source === 'acme') return t('certificates.sourceAcme')
  return source
}

export default function CertificatesPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [ownerId, setOwnerId] = useState(SYSTEM_OWNER)
  const [certificatePem, setCertificatePem] = useState('')
  const [privateKeyPem, setPrivateKeyPem] = useState('')
  const [formError, setFormError] = useState('')

  const fetchCertificates = useCallback(
    (params: { page: number; page_size: number }) => getPage<CertificateRow>('/v1/admin/certificates', params),
    [],
  )
  const users = useQuery({
    queryKey: ['admin-users-options'],
    queryFn: () => getPageItems<UserOption>('/v1/admin/users'),
  })

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-certificates'] })
    await queryClient.invalidateQueries({ queryKey: ['admin-certificates-options'] })
  }

  const ownerOptions = useMemo(
    () => [
      { label: t('certificates.systemOwner'), value: SYSTEM_OWNER },
      ...(users.data ?? []).map((user) => ({ label: user.email, value: user.id })),
    ],
    [t, users.data],
  )

  const create = useMutation({
    mutationFn: async () => {
      await apiClient.post('/v1/admin/certificates', {
        owner_user_id: ownerId || null,
        certificate_pem: certificatePem,
        private_key_pem: privateKeyPem,
      })
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editingId) return
      await apiClient.put(`/v1/admin/certificates/${editingId}`, {
        owner_user_id: ownerId || null,
        certificate_pem: certificatePem,
        private_key_pem: privateKeyPem,
      })
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      await invalidate()
    },
    onError: (error) => setFormError(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/certificates/${id}`)
    },
    onSuccess: invalidate,
    onError: (error) => window.alert(translateApiError(t, error)),
  })

  const openCreate = useCallback(() => {
    setEditingId(null)
    setOwnerId(SYSTEM_OWNER)
    setCertificatePem('')
    setPrivateKeyPem('')
    setFormError('')
    setSidebarOpen(true)
  }, [])

  const openEdit = useCallback((row: CertificateRow) => {
    setEditingId(row.id)
    setOwnerId(row.owner_user_id ?? SYSTEM_OWNER)
    setCertificatePem('')
    setPrivateKeyPem('')
    setFormError('')
    setSidebarOpen(true)
  }, [])

  function submitForm() {
    setFormError('')
    const hasPem = certificatePem.trim().length > 0
    const hasKey = privateKeyPem.trim().length > 0
    if (!editingId && (!hasPem || !hasKey)) {
      setFormError(t('certificates.materialRequired'))
      return
    }
    if (editingId && hasPem !== hasKey) {
      setFormError(t('certificates.materialPair'))
      return
    }
    if (editingId) update.mutate()
    else create.mutate()
  }

  const saving = create.isPending || update.isPending

  const columns: Column<CertificateRow>[] = useMemo(
    () => [
      {
        key: 'id',
        title: translateField(t, 'id'),
        width: ENTITY_LIST_COL_ID,
        fixedWidth: true,
        render: (row) => <EntityIdCell id={row.id} />,
      },
      {
        key: 'owner',
        title: t('certificates.owner'),
        width: 220,
        render: (row) => (
          <span className="data-table-cell" title={row.owner_email ?? t('certificates.systemOwner')}>
            {row.owner_email ?? t('certificates.systemOwner')}
          </span>
        ),
      },
      {
        key: 'domains',
        title: t('certificates.domains'),
        render: (row) => (
          <span className="data-table-cell" title={row.domains.join('\n')}>
            {row.domains.join('、') || '—'}
          </span>
        ),
      },
      {
        key: 'not_after',
        title: t('certificates.notAfter'),
        width: 180,
        render: (row) => {
          const expired = Date.parse(row.not_after) <= Date.now()
          return (
            <span className={`data-table-cell${expired ? ' text-apple-red' : ''}`} title={formatDateTime(row.not_before)}>
              {formatDateTime(row.not_after)}
              {expired ? ` · ${t('certificates.expired')}` : ''}
            </span>
          )
        },
      },
      {
        key: 'source',
        title: t('certificates.source'),
        width: 100,
        render: (row) => <span className="data-table-cell">{sourceLabel(t, row.source)}</span>,
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
              className="data-table-link-btn data-table-link-btn--danger"
              onClick={() => {
                if (window.confirm(t('certificates.deleteConfirm'))) remove.mutate(row.id)
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ],
    [openEdit, remove, t],
  )

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        title={t('pages.certificates')}
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('forms.createCertificate')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-certificates'] })}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        listLayoutId="certificates"
        columns={columns}
        queryKey={['admin-certificates']}
        fetchPage={fetchCertificates}
        rowKey={(row) => row.id}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('forms.editCertificate') : t('forms.createCertificate')}
        onClose={() => setSidebarOpen(false)}
        initialWidth={640}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saving} onClick={submitForm}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          {formError ? <p className="sidebar-form-error" role="alert">{formError}</p> : null}
          <FormField label={t('certificates.owner')}>
            <Select
              value={ownerId}
              options={ownerOptions}
              placeholder={t('certificates.ownerPlaceholder')}
              searchable
              onChange={(value) => {
                setOwnerId(String(value))
                setFormError('')
              }}
            />
          </FormField>
          <FormField label={t('certificates.pem')} required={!editingId} hint={editingId ? t('certificates.pemHint') : t('certificates.domainsParsed')}>
            <textarea
              className="apple-input w-full min-h-[140px] font-mono text-xs"
              value={certificatePem}
              onChange={(event) => {
                setCertificatePem(event.target.value)
                setFormError('')
              }}
              spellCheck={false}
              placeholder="-----BEGIN CERTIFICATE-----"
            />
          </FormField>
          <FormField label={t('certificates.key')} required={!editingId}>
            <textarea
              className="apple-input w-full min-h-[140px] font-mono text-xs"
              value={privateKeyPem}
              onChange={(event) => {
                setPrivateKeyPem(event.target.value)
                setFormError('')
              }}
              spellCheck={false}
              placeholder="-----BEGIN PRIVATE KEY-----"
            />
          </FormField>
        </FormStack>
      </GenericSidebar>
    </section>
  )
}
