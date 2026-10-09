/** ISO 3166-1 alpha-2（不含 EU/UN 等非国家/地区代码） */
const EXCLUDED_REGIONS = new Set([
  'EU', 'EZ', 'UN', 'QO', 'XD', 'XL', 'XP', 'XR', 'XS', 'XT', 'XU', 'XV', 'XW', 'XZ', 'ZZ',
])

/** flagcdn.com，与 SmsGo / Komari 前端一致 */
export function countryFlagUrl(isoAlpha2: string): string {
  const code = isoAlpha2.trim().toLowerCase()
  if (!/^[a-z]{2}$/.test(code)) return ''
  return `https://flagcdn.com/${code}.svg`
}

export function getCountryDisplayName(code: string, locale: string): string {
  const upper = code.trim().toUpperCase()
  if (!upper) return ''
  try {
    const name = new Intl.DisplayNames([locale], { type: 'region' }).of(upper)
    return name && name !== upper ? name : upper
  } catch {
    return upper
  }
}

function regionsFromIntl(): string[] | null {
  if (typeof Intl === 'undefined' || !('supportedValuesOf' in Intl)) return null
  try {
    // TS lib 可能未包含 'region'，运行时（ES2022+）支持
    const supportedValuesOf = Intl.supportedValuesOf as unknown as (key: string) => string[]
    return supportedValuesOf('region')
      .filter((code) => /^[A-Z]{2}$/.test(code))
      .filter((code) => !EXCLUDED_REGIONS.has(code))
  } catch {
    return null
  }
}

/** 备用列表（与 Intl 不可用时一致） */
const ISO3166_ALPHA2_FALLBACK = [
  'AD', 'AE', 'AF', 'AG', 'AI', 'AL', 'AM', 'AO', 'AQ', 'AR', 'AS', 'AT', 'AU', 'AW', 'AX', 'AZ',
  'BA', 'BB', 'BD', 'BE', 'BF', 'BG', 'BH', 'BI', 'BJ', 'BL', 'BM', 'BN', 'BO', 'BQ', 'BR', 'BS', 'BT', 'BV', 'BW', 'BY', 'BZ',
  'CA', 'CC', 'CD', 'CF', 'CG', 'CH', 'CI', 'CK', 'CL', 'CM', 'CN', 'CO', 'CR', 'CU', 'CV', 'CW', 'CX', 'CY', 'CZ',
  'DE', 'DJ', 'DK', 'DM', 'DO', 'DZ',
  'EC', 'EE', 'EG', 'EH', 'ER', 'ES', 'ET',
  'FI', 'FJ', 'FK', 'FM', 'FO', 'FR',
  'GA', 'GB', 'GD', 'GE', 'GF', 'GG', 'GH', 'GI', 'GL', 'GM', 'GN', 'GP', 'GQ', 'GR', 'GS', 'GT', 'GU', 'GW', 'GY',
  'HK', 'HM', 'HN', 'HR', 'HT', 'HU',
  'ID', 'IE', 'IL', 'IM', 'IN', 'IO', 'IQ', 'IR', 'IS', 'IT',
  'JE', 'JM', 'JO', 'JP',
  'KE', 'KG', 'KH', 'KI', 'KM', 'KN', 'KP', 'KR', 'KW', 'KY', 'KZ',
  'LA', 'LB', 'LC', 'LI', 'LK', 'LR', 'LS', 'LT', 'LU', 'LV', 'LY',
  'MA', 'MC', 'MD', 'ME', 'MF', 'MG', 'MH', 'MK', 'ML', 'MM', 'MN', 'MO', 'MP', 'MQ', 'MR', 'MS', 'MT', 'MU', 'MV', 'MW', 'MX', 'MY', 'MZ',
  'NA', 'NC', 'NE', 'NF', 'NG', 'NI', 'NL', 'NO', 'NP', 'NR', 'NU', 'NZ',
  'OM',
  'PA', 'PE', 'PF', 'PG', 'PH', 'PK', 'PL', 'PM', 'PN', 'PR', 'PS', 'PT', 'PW', 'PY',
  'QA',
  'RE', 'RO', 'RS', 'RU', 'RW',
  'SA', 'SB', 'SC', 'SD', 'SE', 'SG', 'SH', 'SI', 'SJ', 'SK', 'SL', 'SM', 'SN', 'SO', 'SR', 'SS', 'ST', 'SV', 'SX', 'SY', 'SZ',
  'TC', 'TD', 'TF', 'TG', 'TH', 'TJ', 'TK', 'TL', 'TM', 'TN', 'TO', 'TR', 'TT', 'TV', 'TW', 'TZ',
  'UA', 'UG', 'UM', 'US', 'UY', 'UZ',
  'VA', 'VC', 'VE', 'VG', 'VI', 'VN', 'VU',
  'WF', 'WS',
  'YE', 'YT',
  'ZA', 'ZM', 'ZW',
]

let cachedCodes: string[] | null = null

export function listIso3166Alpha2(): string[] {
  if (cachedCodes) return cachedCodes
  const fromIntl = regionsFromIntl()
  cachedCodes = fromIntl && fromIntl.length > 0 ? fromIntl.sort() : [...ISO3166_ALPHA2_FALLBACK]
  return cachedCodes
}

export function isValidIso3166Alpha2(code: string): boolean {
  const upper = code.trim().toUpperCase()
  if (!/^[A-Z]{2}$/.test(upper)) return false
  return listIso3166Alpha2().includes(upper)
}
