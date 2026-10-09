import i18n from 'i18next'
import LanguageDetector from 'i18next-browser-languagedetector'
import { initReactI18next } from 'react-i18next'
import en from './locales/en.json'
import zhCN from './locales/zh-CN.json'

export const supportedLanguages = ['zh-CN', 'en'] as const
export type AppLanguage = (typeof supportedLanguages)[number]

/** 仅支持简体与英文；其他系统语言（含繁体）回退到 English */
export function resolveLanguage(input: string): AppLanguage {
  const normalized = input.trim().toLowerCase().replace(/_/g, '-')
  if (normalized.startsWith('zh')) {
    if (
      normalized.includes('tw')
      || normalized.includes('hk')
      || normalized.includes('mo')
      || normalized.includes('hant')
    ) {
      return 'en'
    }
    return 'zh-CN'
  }
  if (normalized.startsWith('en')) {
    return 'en'
  }
  return 'en'
}

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      'zh-CN': { translation: zhCN },
    },
    supportedLngs: [...supportedLanguages],
    fallbackLng: 'en',
    interpolation: { escapeValue: false },
    detection: {
      order: ['navigator'],
      caches: [],
      convertDetectedLanguage: (language) => resolveLanguage(language),
    },
  })

i18n.on('languageChanged', (language) => {
  document.documentElement.lang = language.startsWith('zh') ? 'zh-CN' : 'en'
})

document.documentElement.lang = resolveLanguage(navigator.language || 'en') === 'zh-CN' ? 'zh-CN' : 'en'

export default i18n
