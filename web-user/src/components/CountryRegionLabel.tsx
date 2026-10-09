import { countryFlagUrl, getCountryDisplayName } from '@/utils/iso3166'
import './CountryRegionLabel.css'

export function CountryRegionLabel({
  code,
  locale,
  showCode = false,
  className,
}: {
  code: string
  locale: string
  showCode?: boolean
  className?: string
}) {
  const upper = code.trim().toUpperCase()
  if (!upper) return <span className="text-muted-foreground">—</span>
  const flag = countryFlagUrl(upper)
  const name = getCountryDisplayName(upper, locale)
  return (
    <span className={className ? `region-option ${className}` : 'region-option'}>
      {flag ? <img src={flag} alt="" className="region-option__flag" loading="lazy" /> : null}
      <span className="region-option__text">
        {name}
        {showCode ? <span className="region-option__code">{upper}</span> : null}
      </span>
    </span>
  )
}
