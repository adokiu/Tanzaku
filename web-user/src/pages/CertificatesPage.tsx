import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage } from '@/api/page'
import { Button } from '@/components/Button'
import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { CertExpiryTag } from '@/components/CertExpiryTag'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { formatDateYmd } from '@/utils/formatDateTime'

type CertificateRow = {
  id: string
  owner_user_id: string | null
  domains: string[]
  not_before: string
  not_after: string
  issuer: string
  source: string
}

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
  const [certificatePem, setCertificatePem] = useState('')
  const [privateKeyPem, setPrivateKeyPem] = useState('')

  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<CertificateRow>('/v1/certificates', params),
    [],
  )

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['user-certificates'] })
    await queryClient.invalidateQueries({ queryKey: ['user-certificates-options'] })
  }

  const create = useMutation({
    mutationFn: async () => {
      await apiClient.post('/v1/certificates', {
        certificate_pem: certificatePem,
        private_key_pem: privateKeyPem,
      })
    },
    onSuccess: async () => {
      toast.success(t('certificates.createOk'))
      setSidebarOpen(false)
      setCertificatePem('')
      setPrivateKeyPem('')
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const update = useMutation({
    mutationFn: async () => {
      if (!editingId) return
      await apiClient.put(`/v1/certificates/${editingId}`, {
        certificate_pem: certificatePem,
        private_key_pem: privateKeyPem,
      })
    },
    onSuccess: async () => {
      toast.success(t('common.saveSuccess'))
      setSidebarOpen(false)
      setEditingId(null)
      setCertificatePem('')
      setPrivateKeyPem('')
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => apiClient.delete(`/v1/certificates/${id}`),
    onSuccess: async () => {
      toast.success(t('certificates.deleteOk'))
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const openCreate = useCallback(() => {
    setEditingId(null)
    setCertificatePem('')
    setPrivateKeyPem('')
    setSidebarOpen(true)
  }, [])

  const openEdit = useCallback((row: CertificateRow) => {
    setEditingId(row.id)
    setCertificatePem('')
    setPrivateKeyPem('')
    setSidebarOpen(true)
  }, [])

  function submitForm() {
    const hasPem = certificatePem.trim().length > 0
    const hasKey = privateKeyPem.trim().length > 0
    if (!editingId && (!hasPem || !hasKey)) {
      toast.error(t('certificates.materialRequired'))
      return
    }
    if (editingId && hasPem !== hasKey) {
      toast.error(t('certificates.materialPair'))
      return
    }
    if (editingId) update.mutate()
    else create.mutate()
  }

  const saving = create.isPending || update.isPending

  const columns = useMemo<Column<CertificateRow>[]>(
    () => [
      {
        key: 'domains',
        title: t('certificates.domains'),
        render: (row) => (
          <CellTooltip tip={row.domains?.join('\n') || undefined} className="data-table-cell">
            {row.domains?.join('、') || '—'}
          </CellTooltip>
        ),
      },
      {
        key: 'issuer',
        title: t('certificates.issuer'),
        width: 200,
        render: (row) => (
          <CellTooltip tip={row.issuer || undefined} className="data-table-cell">
            {row.issuer?.trim() || '—'}
          </CellTooltip>
        ),
      },
      {
        key: 'not_after',
        title: t('certificates.notAfter'),
        width: 120,
        fixedWidth: true,
        render: (row) => (
          <CellTooltip
            tip={`${t('certificates.notBefore')}: ${formatDateYmd(row.not_before) || '—'}`}
            className="data-table-cell"
          >
            <CertExpiryTag notAfter={row.not_after} />
          </CellTooltip>
        ),
      },
      {
        key: 'source',
        title: t('certificates.source'),
        width: 100,
        fixedWidth: true,
        render: (row) => (
          <span
            className={`data-table-tag entity-list-cell__status-tag ${
              row.source === 'acme' ? 'data-table-tag--cert-acme' : 'data-table-tag--cert-manual'
            }`}
          >
            {sourceLabel(t, row.source)}
          </span>
        ),
      },
      {
        key: 'owner',
        title: t('certificates.owner'),
        width: 140,
        render: (row) => (
          <span className="data-table-cell">
            {row.owner_user_id ? t('certificates.mine') : t('certificates.systemOwner')}
          </span>
        ),
      },
      {
        key: 'actions',
        title: t('common.actions'),
        render: (row) =>
          row.owner_user_id ? (
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
          ) : (
            '—'
          ),
      },
    ],
    [openEdit, remove, t],
  )

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.certificates')}
        actions={(
          <>
            <Button variant="secondary" size="sm" onClick={() => void invalidate()}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('certificates.upload')}
            </Button>
          </>
        )}
      />
      <EntityList
        columns={columns}
        queryKey={['user-certificates']}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
        emptyText={t('certificates.empty')}
      />
      <GenericSidebar
        open={sidebarOpen}
        onClose={() => {
          setSidebarOpen(false)
          setEditingId(null)
        }}
        title={editingId ? t('forms.editCertificate') : t('certificates.upload')}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={saving} onClick={submitForm}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          <FormField
            label={t('certificates.certPem')}
            required={!editingId}
            hint={editingId ? t('certificates.pemHint') : t('certificates.domainsParsed')}
          >
            <textarea
              className="w-full min-h-32 rounded-md border bg-background p-2 text-sm font-mono"
              value={certificatePem}
              onChange={(e) => setCertificatePem(e.target.value)}
              spellCheck={false}
              placeholder="-----BEGIN CERTIFICATE-----"
            />
          </FormField>
          <FormField label={t('certificates.keyPem')} required={!editingId}>
            <textarea
              className="w-full min-h-32 rounded-md border bg-background p-2 text-sm font-mono"
              value={privateKeyPem}
              onChange={(e) => setPrivateKeyPem(e.target.value)}
              spellCheck={false}
              placeholder="-----BEGIN PRIVATE KEY-----"
            />
          </FormField>
        </FormStack>
      </GenericSidebar>
    </section>
  )
}
