import { useEffect, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { DataTable, type Column } from '@/components/DataTable'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import type { EntityListLayoutId } from '@/utils/entityListLayout'

export type PaginatedListResponse<T> = {
  items: T[]
  total: number
  page: number
  page_size: number
}

type EntityListProps<T> = {
  listLayoutId?: EntityListLayoutId
  columns: Column<T>[]
  queryKey: readonly unknown[]
  fetchPage: (params: { page: number; page_size: number }) => Promise<PaginatedListResponse<T>>
  rowKey: (row: T) => string
  refetchInterval?: number | false
  enabled?: boolean
  /** 变化时重置到第 1 页（例如切换筛选） */
  resetPageWhen?: unknown
  emptyText?: string
  tableClassName?: string
}

export function EntityList<T>({
  listLayoutId,
  columns,
  queryKey,
  fetchPage,
  rowKey,
  refetchInterval = false,
  enabled = true,
  resetPageWhen,
  emptyText,
  tableClassName = 'data-table--entity-list',
}: EntityListProps<T>) {
  const { t } = useTranslation()
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)

  useEffect(() => {
    setPage(1)
  }, [resetPageWhen])

  const query = useQuery({
    queryKey: [...queryKey, page, pageSize],
    queryFn: () => fetchPage({ page, page_size: pageSize }),
    refetchInterval,
    enabled,
  })

  const payload = query.data
  const total = payload?.total ?? 0

  if (query.isError) {
    return (
      <p className="page-card p-5 text-sm text-apple-red" role="alert">
        {translateApiError(t, query.error)}
      </p>
    )
  }

  return (
    <DataTable
      tableClassName={tableClassName}
      listLayoutId={listLayoutId}
      columns={columns}
      data={payload?.items ?? []}
      rowKey={rowKey}
      loading={query.isPending}
      emptyText={emptyText ?? t('common.empty')}
      pagination={{ page, size: pageSize, total }}
      onPageChange={setPage}
      onSizeChange={(size) => {
        setPageSize(size)
        setPage(1)
      }}
    />
  )
}

type EntityListPageProps<T> = EntityListProps<T> & {
  title: string
  actions?: ReactNode
  toolbar?: ReactNode
}

export function EntityListPage<T>({
  title,
  actions,
  toolbar,
  ...list
}: EntityListPageProps<T>) {
  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader title={title} actions={actions} />
      {toolbar ? <div className="entity-list-toolbar">{toolbar}</div> : null}
      <EntityList {...list} />
    </section>
  )
}
