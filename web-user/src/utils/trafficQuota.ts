export type TrafficQuotaUnit = 'MB' | 'GB' | 'TB' | 'PB' | 'unlimited'

const UNIT_BYTES: Record<Exclude<TrafficQuotaUnit, 'unlimited'>, number> = {
  MB: 1024 ** 2,
  GB: 1024 ** 3,
  TB: 1024 ** 4,
  PB: 1024 ** 5,
}

const UNIT_ORDER: Exclude<TrafficQuotaUnit, 'unlimited'>[] = ['PB', 'TB', 'GB', 'MB']

export function trafficQuotaToBytes(
  value: string,
  unit: TrafficQuotaUnit,
): number | null {
  if (unit === 'unlimited') return null
  const trimmed = value.trim()
  if (!trimmed) return null
  const amount = Number(trimmed)
  if (!Number.isFinite(amount) || amount <= 0) return null
  const bytes = Math.round(amount * UNIT_BYTES[unit])
  if (bytes <= 0) return null
  return bytes
}

/** 将字节数拆成便于编辑的数值与单位（优先 TB/GB） */
export function bytesToTrafficQuotaDisplay(bytes: number | null | undefined): {
  value: string
  unit: TrafficQuotaUnit
} {
  if (bytes == null || bytes <= 0) {
    return { value: '', unit: 'unlimited' }
  }
  for (const unit of UNIT_ORDER) {
    const chunk = UNIT_BYTES[unit]
    if (bytes >= chunk && bytes % chunk === 0) {
      return { value: String(bytes / chunk), unit }
    }
  }
  const gb = UNIT_BYTES.GB
  if (bytes >= gb) {
    const n = bytes / gb
    return { value: n >= 100 ? String(Math.round(n)) : n.toFixed(2).replace(/\.?0+$/, ''), unit: 'GB' }
  }
  const mb = UNIT_BYTES.MB
  const n = bytes / mb
  return { value: n >= 100 ? String(Math.round(n)) : n.toFixed(2).replace(/\.?0+$/, ''), unit: 'MB' }
}

export type TrafficAmountUnit = Exclude<TrafficQuotaUnit, 'unlimited'>

/** 已用流量等允许为 0 的数值（不含「不限」）。 */
export function trafficAmountToBytes(value: string, unit: TrafficAmountUnit): number | null {
  const trimmed = value.trim()
  if (!trimmed) return null
  const amount = Number(trimmed)
  if (!Number.isFinite(amount) || amount < 0) return null
  return Math.round(amount * UNIT_BYTES[unit])
}

export function bytesToTrafficAmountDisplay(bytes: number | null | undefined): {
  value: string
  unit: TrafficAmountUnit
} {
  if (!bytes || bytes <= 0) return { value: '0', unit: 'GB' }
  const display = bytesToTrafficQuotaDisplay(bytes)
  return display.unit === 'unlimited'
    ? { value: '0', unit: 'GB' }
    : { value: display.value, unit: display.unit }
}

export function formatTrafficQuotaLabel(
  bytes: number | null | undefined,
  unlimitedLabel: string,
): string {
  if (bytes == null || bytes <= 0) return unlimitedLabel
  const { value, unit } = bytesToTrafficQuotaDisplay(bytes)
  if (unit === 'unlimited') return unlimitedLabel
  return `${value} ${unit}`
}
