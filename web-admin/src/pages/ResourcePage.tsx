import { useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { translateField } from '@/i18n/fieldLabel'

interface PageData {
  items?: Record<string, unknown>[]
  total?: number
  page?: number
  page_size?: number
}

export default function ResourcePage({ endpoint }: { endpoint: string }) {
  const { t, i18n } = useTranslation()
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const query = useQuery({
    queryKey: ['resource', endpoint, page, pageSize, i18n.language],
    queryFn: async () => (await apiClient.get(endpoint, { params: { page, page_size: pageSize } })).data as PageData | Record<string, unknown>[],
  })
  const payload = query.data
  const rows: Record<string, unknown>[] = Array.isArray(payload)
    ? payload
    : payload && typeof payload === 'object'
      ? Array.isArray(payload.items)
        ? payload.items
        : [payload as Record<string, unknown>]
      : []
  const columns: Column<Record<string, unknown>>[] = useMemo(
    () => Object.keys(rows[0] ?? {}).map((key) => ({ key, title: translateField(t, key) })),
    [rows, t],
  )
  const isPage = Boolean(payload && !Array.isArray(payload) && Array.isArray(payload.items))
  const total = isPage && payload && !Array.isArray(payload) && typeof payload.total === 'number' ? payload.total : rows.length

  return (
    <section className="page-container">
      <PageListHeader
        actions={(
          <Button variant="secondary" size="sm" onClick={() => query.refetch()} disabled={query.isFetching}>
            <RefreshCw size={16} className={query.isFetching ? 'animate-spin' : ''} />
            {t('common.refresh')}
          </Button>
        )}
      />
      {query.isError ? <div className="page-card p-6 text-sm text-apple-red" role="alert">{translateApiError(t, query.error)}</div> : (
        <>
          <DataTable
            columns={columns}
            data={rows}
            rowKey={(row) => String(row.id ?? JSON.stringify(row))}
            loading={query.isPending}
            emptyText={t('common.empty')}
            pagination={isPage ? { page, size: pageSize, total } : undefined}
            onPageChange={setPage}
            onSizeChange={(size) => {
              setPageSize(size)
              setPage(1)
            }}
          />
        </>
      )}
    </section>
  )
}
