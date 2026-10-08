import type { TFunction } from 'i18next'

const KNOWN_API_ERRORS: Record<string, string> = {
  '中国大陆节点仅允许中国大陆地区的 Client 连接，以降低跨境数据传输风险': 'errors.cnResidencyBlocked',
  '已达到该节点允许的隧道数量上限': 'errors.nodeTunnelLimit',
}

export function translateApiError(t: TFunction, error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    const message = response?.data?.error
    if (message) {
      const key = KNOWN_API_ERRORS[message]
      if (key) return t(key)
      return message
    }
  }
  return t('errors.boardUnreachable')
}
