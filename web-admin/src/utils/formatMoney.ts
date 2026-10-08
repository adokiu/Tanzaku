/** 余额等单位：数据库存分（int64），展示为元，如 238 → 2.38¥ */
export function formatBalanceCents(cents: number): string {
  if (!Number.isFinite(cents)) return '0.00¥'
  const yuan = cents / 100
  const sign = yuan < 0 ? '-' : ''
  return `${sign}${Math.abs(yuan).toFixed(2)}¥`
}

/** 表单输入元（如 2.38）→ 分（238）；无效返回 null */
export function parseBalanceYuanInput(value: string): number | null {
  const trimmed = value.trim()
  if (!trimmed) return 0
  const yuan = Number(trimmed)
  if (!Number.isFinite(yuan)) return null
  return Math.round(yuan * 100)
}

export function balanceYuanFromCents(cents: number): string {
  if (!Number.isFinite(cents)) return '0.00'
  return (cents / 100).toFixed(2)
}
