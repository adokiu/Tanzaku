import { create } from 'zustand'

export type ToastType = 'success' | 'error' | 'info'

export type ToastItem = {
  id: string
  type: ToastType
  message: string
  duration: number
}

type ToastState = {
  items: ToastItem[]
  push: (input: { type: ToastType; message: string; duration?: number }) => string
  dismiss: (id: string) => void
  clear: () => void
}

let seq = 0

export const useToastStore = create<ToastState>((set) => ({
  items: [],
  push: ({ type, message, duration }) => {
    const id = `toast-${Date.now()}-${++seq}`
    const item: ToastItem = {
      id,
      type,
      message: message.trim() || '…',
      duration: duration ?? (type === 'error' ? 5200 : 3200),
    }
    set((state) => ({ items: [...state.items.slice(-4), item] }))
    return id
  },
  dismiss: (id) => set((state) => ({ items: state.items.filter((item) => item.id !== id) })),
  clear: () => set({ items: [] }),
}))

export const toast = {
  success: (message: string, duration?: number) =>
    useToastStore.getState().push({ type: 'success', message, duration }),
  error: (message: string, duration?: number) =>
    useToastStore.getState().push({ type: 'error', message, duration }),
  info: (message: string, duration?: number) =>
    useToastStore.getState().push({ type: 'info', message, duration }),
  dismiss: (id: string) => useToastStore.getState().dismiss(id),
}
