import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Select } from '@/components/Select/Select'
import { CountryRegionLabel } from '@/components/CountryRegionLabel'
import { getCountryDisplayName, listIso3166Alpha2 } from '@/utils/iso3166'

export function CountryRegionSelect({
  value,
  onChange,
  placeholder,
  disabled,
}: {
  value: string
  onChange: (code: string) => void
  placeholder?: string
  disabled?: boolean
}) {
  const { i18n, t } = useTranslation()
  const locale = i18n.language

  const options = useMemo(() => {
    const codes = listIso3166Alpha2()
    return codes
      .map((code) => {
        const name = getCountryDisplayName(code, locale)
        return {
          value: code,
          label: <CountryRegionLabel code={code} locale={locale} />,
          searchText: `${code} ${name}`.toLowerCase(),
        }
      })
      .sort((a, b) => {
        const na = getCountryDisplayName(String(a.value), locale)
        const nb = getCountryDisplayName(String(b.value), locale)
        return na.localeCompare(nb, locale)
      })
  }, [locale])

  return (
    <Select
      value={value || undefined}
      options={options}
      placeholder={placeholder ?? t('select.region')}
      disabled={disabled}
      searchable
      emptyText={t('common.empty')}
      onChange={(next) => onChange(String(next).toUpperCase())}
    />
  )
}
