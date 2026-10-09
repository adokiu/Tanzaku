import { readThemeSettings, useThemeSettingsStore } from '@/stores/themeSettings'
import './SiteFooter.css'

export function SiteFooter() {
  const settings = useThemeSettingsStore((state) => state.settings)
  const parsed = readThemeSettings(settings)
  const showIcp = parsed.icpEnabled && Boolean(parsed.icpNumber)
  const showPolice = parsed.policeEnabled && Boolean(parsed.policeNumber)
  if (!showIcp && !showPolice) return null

  return (
    <footer className="site-footer">
      <div className="site-footer__inner">
        {showIcp ? (
          parsed.icpUrl ? (
            <a className="site-footer__link" href={parsed.icpUrl} target="_blank" rel="noopener noreferrer">
              {parsed.icpNumber}
            </a>
          ) : (
            <span>{parsed.icpNumber}</span>
          )
        ) : null}
        {showPolice ? (
          parsed.policeUrl ? (
            <a className="site-footer__link" href={parsed.policeUrl} target="_blank" rel="noopener noreferrer">
              {parsed.policeNumber}
            </a>
          ) : (
            <span>{parsed.policeNumber}</span>
          )
        ) : null}
      </div>
    </footer>
  )
}
