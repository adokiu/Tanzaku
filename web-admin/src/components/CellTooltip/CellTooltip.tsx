import { useCallback, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import './CellTooltip.css'

type CellTooltipProps = {
  tip?: string | null
  children: ReactNode
  className?: string
  /** 传给外层包裹元素，默认 span */
  as?: 'span' | 'div'
}

/**
 * 列表单元格悬停提示：立即显示，自定义样式；用 portal 避免被表格 overflow 裁切。
 */
export function CellTooltip({ tip, children, className = '', as = 'span' }: CellTooltipProps) {
  const text = tip?.trim() ?? ''
  const ref = useRef<HTMLElement>(null)
  const [open, setOpen] = useState(false)
  const [style, setStyle] = useState<CSSProperties>({})

  const show = useCallback(() => {
    if (!text) return
    const rect = ref.current?.getBoundingClientRect()
    if (!rect) return
    const pad = 8
    const maxW = 320
    let left = rect.left + rect.width / 2
    left = Math.min(Math.max(left, pad + maxW / 2), window.innerWidth - pad - maxW / 2)
    const placeBelow = rect.top < 56
    setStyle(
      placeBelow
        ? { left, top: rect.bottom + pad, transform: 'translate(-50%, 0)' }
        : { left, top: rect.top - pad, transform: 'translate(-50%, -100%)' },
    )
    setOpen(true)
  }, [text])

  const hide = useCallback(() => setOpen(false), [])

  const Tag = as
  const wrapClass = ['cell-tooltip', className].filter(Boolean).join(' ')

  if (!text) {
    return <Tag className={wrapClass || undefined}>{children}</Tag>
  }

  return (
    <>
      <Tag
        ref={ref as never}
        className={wrapClass}
        onMouseEnter={show}
        onMouseLeave={hide}
        onFocus={show}
        onBlur={hide}
      >
        {children}
      </Tag>
      {open
        ? createPortal(
            <div className="cell-tooltip__bubble" style={style} role="tooltip">
              {text}
            </div>,
            document.body,
          )
        : null}
    </>
  )
}
