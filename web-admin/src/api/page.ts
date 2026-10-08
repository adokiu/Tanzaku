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
