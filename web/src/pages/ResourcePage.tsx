import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { DataTable, type Column } from '@/components/DataTable'

interface PageData {
  items?: Record<string, unknown>[]
  total?: number
  page?: number
  page_size?: number
}

export default function ResourcePage({ title, endpoint }: { title: string; endpoint: string }) {
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const query = useQuery({
    queryKey: ['resource', endpoint, page, pageSize],
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
  const columns: Column<Record<string, unknown>>[] = Object.keys(rows[0] ?? {})
    .filter((key) => {
      const value = rows[0]?.[key]
      return value == null || typeof value !== 'object'
    })
    .map((key) => ({ key, title: key }))
  const isPage = Boolean(payload && !Array.isArray(payload) && Array.isArray(payload.items))
  const total = isPage && payload && !Array.isArray(payload) && typeof payload.total === 'number' ? payload.total : rows.length

  return (
    <section className="page-container">
      <header className="page-header"><h1 className="page-title">{title}</h1></header>
      {query.isError ? <div className="page-card p-6 text-sm text-apple-red" role="alert">{apiError(query.error)}</div> : (
        <>
          <DataTable
            columns={columns}
            data={rows}
            rowKey={(row) => String(row.id ?? JSON.stringify(row))}
            loading={query.isPending}
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

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
