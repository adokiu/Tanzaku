import type { ReactNode } from 'react'

export function PageListHeader({ title, actions }: { title: ReactNode; actions?: ReactNode }) {
  return (
    <div className="page-header">
      <h1 className="page-title">{title}</h1>
      {actions ? (
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>{actions}</div>
      ) : null}
    </div>
  )
}
