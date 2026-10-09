import type { ReactNode } from 'react'

export function FormField({
  label,
  required,
  hint,
  children,
}: {
  label: string
  required?: boolean
  hint?: string
  children: ReactNode
}) {
  return (
    <div>
      <label style={{ fontSize: 13, fontWeight: 600, display: 'block', marginBottom: 6 }}>
        {label}
        {required ? <span style={{ color: 'hsl(var(--destructive))' }}> *</span> : null}
      </label>
      {hint ? (
        <p style={{ fontSize: 12, color: 'hsl(var(--muted-foreground))', margin: '0 0 6px' }}>{hint}</p>
      ) : null}
      {children}
    </div>
  )
}

export function FormStack({ children }: { children: ReactNode }) {
  return <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>{children}</div>
}
