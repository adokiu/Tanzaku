import { create } from 'zustand'
import apiClient from '@/api/client'

export interface SiteBranding {
  site_title: string
  site_subtitle: string
  site_description: string
  site_url: string
}

interface BrandingState {
  branding: SiteBranding | null
  loaded: boolean
  load: (force?: boolean) => Promise<SiteBranding>
}

const EMPTY: SiteBranding = {
  site_title: '',
  site_subtitle: '',
  site_description: '',
  site_url: '',
}

export function displaySiteTitle(branding: SiteBranding | null | undefined, fallback = 'Tanzaku'): string {
  const title = branding?.site_title?.trim()
  return title || fallback
}

export const useBrandingStore = create<BrandingState>((set, get) => ({
  branding: null,
  loaded: false,
  load: async (force = false) => {
    if (!force && get().loaded && get().branding) {
      return get().branding!
    }
    try {
      const response = await apiClient.get('/public/branding')
      const data = response.data as Partial<SiteBranding>
      const branding: SiteBranding = {
        site_title: typeof data.site_title === 'string' ? data.site_title : '',
        site_subtitle: typeof data.site_subtitle === 'string' ? data.site_subtitle : '',
        site_description: typeof data.site_description === 'string' ? data.site_description : '',
        site_url: typeof data.site_url === 'string' ? data.site_url : '',
      }
      set({ branding, loaded: true })
      return branding
    } catch {
      set({ branding: EMPTY, loaded: true })
      return EMPTY
    }
  },
}))
