import { useCallback, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import apiClient from '@/api/client'

export type PageResult<T> = {
  items: T[]
  total: number
  page: number
  page_size: number
}

export async function getPage<T>(url: string, params?: Record<string, unknown>): Promise<PageResult<T>> {
  const response = await apiClient.get(url, { params })
  return response.data as PageResult<T>
}

export async function getPageItems<T>(url: string, params?: Record<string, unknown>): Promise<T[]> {
  const page = await getPage<T>(url, { page: 1, page_size: 100, ...params })
  return page.items ?? []
}

export function usePagedList<T>(queryKey: unknown[], url: string, extraParams?: Record<string, unknown>) {
  const [page, setPage] = useState(1)
  const [size, setSize] = useState(20)
  const query = useQuery({
    queryKey: [...queryKey, page, size, extraParams],
    queryFn: () => getPage<T>(url, { page, page_size: size, ...extraParams }),
  })
  const setSizeAndReset = useCallback((next: number) => {
    setSize(next)
    setPage(1)
  }, [])
  return {
    query,
    items: query.data?.items ?? [],
    pagination: {
      page,
      size,
      total: query.data?.total ?? 0,
    },
    setPage,
    setSize: setSizeAndReset,
  }
}
