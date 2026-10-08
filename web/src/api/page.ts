import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import apiClient from '@/api/client'

export type PageResult<T> = {
  items: T[]
  total: number
  page: number
  page_size: number
}

export function readPage<T>(data: unknown): PageResult<T> {
  if (data && typeof data === 'object' && Array.isArray((data as PageResult<T>).items)) {
    return data as PageResult<T>
  }
  const items = Array.isArray(data) ? (data as T[]) : []
  return { items, total: items.length, page: 1, page_size: items.length || 20 }
}

export async function getPageItems<T>(url: string, params?: Record<string, unknown>): Promise<T[]> {
  const response = await apiClient.get(url, { params: { page: 1, page_size: 100, ...params } })
  return readPage<T>(response.data).items
}

export function usePagedList<T>(queryKey: readonly unknown[], endpoint: string) {
  const [page, setPage] = useState(1)
  const [size, setSize] = useState(20)
  const query = useQuery({
    queryKey: [...queryKey, page, size],
    queryFn: async () => {
      const response = await apiClient.get(endpoint, { params: { page, page_size: size } })
      return readPage<T>(response.data)
    },
  })
  return {
    query,
    items: query.data?.items ?? [],
    pagination: { page, size, total: query.data?.total ?? 0 },
    setPage,
    setSize: (next: number) => {
      setSize(next)
      setPage(1)
    },
  }
}
