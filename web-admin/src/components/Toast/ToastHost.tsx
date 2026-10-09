import { useEffect } from 'react'
import { CheckCircle2, Info, X, XCircle } from 'lucide-react'
import { useToastStore, type ToastItem, type ToastType } from '@/stores/toast'
import './Toast.css'

const ICONS: Record<ToastType, typeof CheckCircle2> = {
  success: CheckCircle2,
  error: XCircle,
  info: Info,
}

function ToastCard({ item }: { item: ToastItem }) {
  const dismiss = useToastStore((state) => state.dismiss)
  const Icon = ICONS[item.type]

  useEffect(() => {
    if (item.duration <= 0) return
    const timer = window.setTimeout(() => dismiss(item.id), item.duration)
    return () => window.clearTimeout(timer)
  }, [dismiss, item.duration, item.id])

  return (
    <div className={`toast toast--${item.type}`} role={item.type === 'error' ? 'alert' : 'status'}>
      <Icon className="toast__icon" size={18} strokeWidth={2} aria-hidden />
      <p className="toast__message">{item.message}</p>
      <button
        type="button"
        className="toast__close"
        aria-label="Dismiss"
        onClick={() => dismiss(item.id)}
      >
        <X size={14} strokeWidth={2.25} />
      </button>
    </div>
  )
}

export function ToastHost() {
  const items = useToastStore((state) => state.items)
  if (items.length === 0) return null
  return (
    <div className="toast-host" aria-live="polite" aria-relevant="additions">
      {items.map((item) => (
        <ToastCard key={item.id} item={item} />
      ))}
    </div>
  )
}
