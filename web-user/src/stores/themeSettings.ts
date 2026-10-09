import { create } from 'zustand'
import apiClient from '@/api/client'

export type ThemeSettings = Record<string, unknown>

interface ThemeSettingsState {
  short: string
  version: string
  settings: ThemeSettings
  loaded: boolean
  load: () => Promise<{ short: string; version: string }>
}

function asBool(value: unknown, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback
}

function asString(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function asNumber(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

export function readThemeSettings(settings: ThemeSettings) {
  const backgroundEnabled = asBool(settings.backgroundEnabled)
  const backgroundBlur = Math.max(0, asNumber(settings.backgroundBlur))
  return {
    icpEnabled: asBool(settings.icpEnabled),
    icpNumber: asString(settings.icpNumber).trim(),
    icpUrl: asString(settings.icpUrl, 'https://beian.miit.gov.cn/').trim() || 'https://beian.miit.gov.cn/',
    policeEnabled: asBool(settings.policeEnabled),
    policeNumber: asString(settings.policeNumber).trim(),
    policeUrl: asString(settings.policeUrl).trim(),
    backgroundEnabled,
    backgroundType: asString(settings.backgroundType, 'image') === 'video' ? 'video' as const : 'image' as const,
    lightBackgroundUrl: asString(settings.lightBackgroundUrl).trim(),
    darkBackgroundUrl: asString(settings.darkBackgroundUrl).trim(),
    backgroundBlur,
    backgroundOverlay: Math.min(100, Math.max(0, asNumber(settings.backgroundOverlay))),
    cardBlurRadius: backgroundEnabled && backgroundBlur > 0 ? backgroundBlur + 8 : 0,
  }
}

export const useThemeSettingsStore = create<ThemeSettingsState>((set) => ({
  short: '',
  version: '',
  settings: {},
  loaded: false,
  load: async () => {
    const response = await apiClient.get('/public/theme')
    const data = response.data as { short?: string; version?: string; settings?: ThemeSettings }
    const short = data.short ?? ''
    const version = data.version ?? ''
    const settings = data.settings && typeof data.settings === 'object' ? data.settings : {}
    set({ short, version, settings, loaded: true })
    return { short, version }
  },
}))
