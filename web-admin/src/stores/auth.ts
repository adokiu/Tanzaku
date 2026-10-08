import { create } from 'zustand'

export interface Account {
  user_id: string
  email: string
  role: 'admin' | 'user'
  csrf_token: string
}

interface AuthState {
  account: Account | null
  setAccount: (account: Account | null) => void
}

export const useAuthStore = create<AuthState>((set) => ({
  account: null,
  setAccount: (account) => set({ account }),
}))
