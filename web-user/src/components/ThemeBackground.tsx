import { useEffect, useMemo, useState } from 'react'
import { useThemeStore } from '@/stores/theme'
import { readThemeSettings, useThemeSettingsStore } from '@/stores/themeSettings'
import './ThemeBackground.css'

function resolveMode(theme: 'light' | 'dark' | 'system'): 'light' | 'dark' {
  if (theme !== 'system') return theme
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

export function ThemeBackground() {
  const theme = useThemeStore((state) => state.theme)
  const settings = useThemeSettingsStore((state) => state.settings)
  const parsed = readThemeSettings(settings)
  const [loaded, setLoaded] = useState(false)
  const [failed, setFailed] = useState(false)
  const [mode, setMode] = useState<'light' | 'dark'>(() => resolveMode(theme))

  useEffect(() => {
    const apply = () => setMode(resolveMode(theme))
    apply()
    if (theme !== 'system') return
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])

  const url = useMemo(() => {
    return mode === 'dark' ? parsed.darkBackgroundUrl : parsed.lightBackgroundUrl
  }, [mode, parsed.darkBackgroundUrl, parsed.lightBackgroundUrl])

  useEffect(() => {
    if (!parsed.backgroundEnabled || !url || parsed.backgroundType !== 'image') {
      setLoaded(false)
      setFailed(false)
      return
    }
    setLoaded(false)
    setFailed(false)
    const image = new Image()
    image.onload = () => {
      setLoaded(true)
      setFailed(false)
    }
    image.onerror = () => {
      setLoaded(false)
      setFailed(true)
    }
    image.src = url
    return () => {
      image.onload = null
      image.onerror = null
    }
  }, [parsed.backgroundEnabled, parsed.backgroundType, url])

  if (!parsed.backgroundEnabled) return null

  const showMedia = Boolean(url) && !failed && (parsed.backgroundType === 'video' || loaded)
  const blur = parsed.backgroundBlur > 0 ? `blur(${parsed.backgroundBlur}px)` : 'none'
  const overlay =
    parsed.backgroundOverlay > 0 ? `rgba(0, 0, 0, ${parsed.backgroundOverlay / 100})` : 'transparent'

  return (
    <div className="theme-background" aria-hidden>
      {showMedia && parsed.backgroundType === 'image' ? (
        <div
          className="theme-background__media"
          style={{ backgroundImage: `url(${url})`, filter: blur }}
        />
      ) : null}
      {url && parsed.backgroundType === 'video' ? (
        <video
          className="theme-background__media"
          style={{ filter: blur, opacity: failed ? 0 : 1 }}
          src={url}
          autoPlay
          muted
          loop
          playsInline
          onLoadedData={() => {
            setLoaded(true)
            setFailed(false)
          }}
          onError={() => {
            setLoaded(false)
            setFailed(true)
          }}
        />
      ) : null}
      <div className="theme-background__overlay" style={{ backgroundColor: overlay }} />
    </div>
  )
}
