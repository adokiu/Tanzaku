import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { Loader2 } from 'lucide-react'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  loading?: boolean
  icon?: ReactNode
  variant?: 'primary' | 'secondary'
}

export function Button({ loading = false, icon, variant = 'primary', children, className = '', disabled, ...props }: ButtonProps) {
  return (
    <button className={`apple-button ${variant === 'secondary' ? 'apple-button-secondary' : ''} ${className}`} disabled={disabled || loading} {...props}>
      {loading ? <Loader2 size={16} className="animate-spin" /> : icon}
      {children}
    </button>
  )
}
