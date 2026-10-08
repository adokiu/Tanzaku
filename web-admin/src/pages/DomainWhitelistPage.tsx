import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPage } from '@/api/page'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { formatDateTime } from '@/utils/formatDateTime'

type WhitelistRow = {
  id: string
  domain: string
  note: string
  created_at: string
}

export default function DomainWhitelistPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [domain, setDomain] = useState('')
  const [note, setNote] = useState('')
  const [message, setMessage] = useState('')

  const fetchPage = useCallback(
    (params: { page: number; page_size: number }) => getPage<WhitelistRow>('/v1/admin/domain-whitelist', params),
    [],
  )

  const create = useMutation({
    mutationFn: async () => apiClient.post('/v1/admin/domain-whitelist', { domain: domain.trim(), note: note.trim() }),
    onSuccess: async () => {
      setDomain('')
      setNote('')
      setMessage(t('domains.whitelisted'))
      await queryClient.invalidateQueries({ queryKey: ['admin-domain-whitelist'] })
    },
    onError: (error) => setMessage(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => apiClient.delete(`/v1/admin/domain-whitelist/${id}`),
    onSuccess: async () => {
      setMessage(t('domains.whitelistRemoved'))
      await queryClient.invalidateQueries({ queryKey: ['admin-domain-whitelist'] })
    },
    onError: (error) => setMessage(translateApiError(t, error)),
  })

  const columns: Column<WhitelistRow>[] = useMemo(() => [
    { key: 'domain', title: t('domains.domain'), render: (row) => <span className="data-table-cell">{row.domain}</span> },
    { key: 'note', title: t('domains.note'), render: (row) => <span className="data-table-cell">{row.note || '—'}</span> },
    {
      key: 'created_at',
      title: t('domains.createdAt'),
      render: (row) => <span className="data-table-cell">{formatDateTime(row.created_at)}</span>,
    },
    {
      key: 'actions',
      title: t('common.delete'),
      render: (row) => (
        <button type="button" className="data-table-link-btn" onClick={() => remove.mutate(row.id)}>
          {t('common.delete')}
        </button>
      ),
    },
  ], [remove, t])

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.domainWhitelist')}
        actions={(
          <Button variant="secondary" size="sm" onClick={() => void queryClient.invalidateQueries({ queryKey: ['admin-domain-whitelist'] })}>
            <RefreshCw size={16} />
            {t('common.refresh')}
          </Button>
        )}
      />
      <form
        className="page-card mb-4 grid gap-3 p-4 md:grid-cols-[1fr_1fr_auto]"
        onSubmit={(event) => {
          event.preventDefault()
          setMessage('')
          create.mutate()
        }}
      >
        <Input value={domain} onChange={(event) => setDomain(event.target.value)} placeholder={t('domains.domainPlaceholder')} required />
        <Input value={note} onChange={(event) => setNote(event.target.value)} placeholder={t('domains.notePlaceholder')} />
        <Button type="submit" loading={create.isPending}>{t('domains.add')}</Button>
      </form>
      <p className="mb-4 text-sm text-muted-foreground">{t('domains.whitelistHint')}</p>
      {message && <p className="mb-4 text-sm text-muted-foreground" role="status">{message}</p>}
      <EntityList
        columns={columns}
        queryKey={['admin-domain-whitelist']}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
        tableClassName=""
      />
    </section>
  )
}
