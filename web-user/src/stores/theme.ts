import { create } from 'zustand'
import { persist } from 'zustand/middleware'

type Theme = 'light' | 'dark' | 'system'

interface ThemeState {
  theme: Theme
  setTheme: (theme: Theme) => void
  apply: () => void
}

function resolveTheme(theme: Theme): 'light' | 'dark' {
  if (theme !== 'system') return theme
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

export const useThemeStore = create<ThemeState>()(persist((set, get) => ({
  theme: 'system',
  setTheme: (theme) => {
    set({ theme })
    document.documentElement.classList.toggle('dark', resolveTheme(theme) === 'dark')
  },
  apply: () => document.documentElement.classList.toggle('dark', resolveTheme(get().theme) === 'dark'),
}), { name: 'tanzaku-theme' }))
