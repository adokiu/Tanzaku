/** 人类可读：2026/10/7 18:53:36（本地时区，月/日不补零，时分秒补零） */
export function formatDateTime(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (value === '') return ''

  const date = parseToDate(value)
  if (!date) return typeof value === 'string' || typeof value === 'number' ? String(value) : ''

  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}/${date.getMonth() + 1}/${date.getDate()} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
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
