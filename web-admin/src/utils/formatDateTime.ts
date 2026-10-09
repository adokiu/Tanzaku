/** 将秒数格式化为可读持续时间（如 `45s` / `3m 12s` / `1h 5m`）。 */
export function formatDurationSecs(secs: unknown): string {
  const n = typeof secs === 'number' ? secs : Number(secs)
  if (!Number.isFinite(n) || n < 0) return '—'
  const total = Math.floor(n)
  if (total < 1) return '<1s'
  if (total < 60) return `${total}s`
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
  if (minutes < 60) return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
  const hours = Math.floor(minutes / 60)
  const remainMinutes = minutes % 60
  return remainMinutes > 0 ? `${hours}h ${remainMinutes}m` : `${hours}h`
}

/** 人类可读：2026/10/7 18:53:36（本地时区，月/日不补零，时分秒补零） */
export function formatDateTime(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (value === '') return ''

  const date = parseToDate(value)
  if (!date) return typeof value === 'string' || typeof value === 'number' ? String(value) : ''

  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}/${date.getMonth() + 1}/${date.getDate()} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/** 仅日期：2026/10/07（本地时区，月日补零） */
export function formatDateYmd(value: unknown): string {
  const date = parseToDate(value)
  if (!date) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`
}

/** 距本地「今天」还有多少整天（按日期）；当天为 0，已过期为负；无效为 null */
export function daysUntilDate(value: unknown): number | null {
  const date = parseToDate(value)
  if (!date) return null
  const now = new Date()
  const expiryDay = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())
  const today = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate())
  return Math.round((expiryDay - today) / 86_400_000)
}

export function isDateTimeFieldKey(key: string): boolean {
  const k = key.toLowerCase()
  if (k === 'timestamp' || k === 'time') return true
  if (k.endsWith('_at') || k.endsWith('_until')) return true
  if (k === 'last_seen' || k === 'not_before' || k === 'not_after') return true
  return false
}

export function shouldFormatAsDateTime(key: string, value: unknown): boolean {
  if (isDateTimeFieldKey(key)) return true
  if (typeof value === 'string') {
    const trimmed = value.trim()
    if (/^\d{4}-\d{2}-\d{2}[T ]/.test(trimmed)) return true
    if (/^\d{4}-\d{2}-\d{2}$/.test(trimmed)) return true
  }
  return false
}

function parseToDate(value: unknown): Date | null {
  if (value instanceof Date) {
    return Number.isNaN(value.getTime()) ? null : value
  }
  if (typeof value === 'number' && Number.isFinite(value)) {
    const ms = value > 1e12 ? value : value * 1000
    const d = new Date(ms)
    return Number.isNaN(d.getTime()) ? null : d
  }
  if (typeof value === 'string') {
    const trimmed = value.trim()
    if (!trimmed) return null
    if (/^\d+$/.test(trimmed)) {
      const n = Number(trimmed)
      if (!Number.isFinite(n)) return null
      const ms = n > 1e12 ? n : n * 1000
      const d = new Date(ms)
      return Number.isNaN(d.getTime()) ? null : d
    }
    const d = new Date(trimmed)
    return Number.isNaN(d.getTime()) ? null : d
  }
  return null
}

export function formatCellDisplay(key: string, value: unknown): string {
  if (value === null || value === undefined) return ''
  if (shouldFormatAsDateTime(key, value)) {
    const formatted = formatDateTime(value)
    if (formatted) return formatted
  }
  if (typeof value === 'object') return JSON.stringify(value)
  return String(value)
}
