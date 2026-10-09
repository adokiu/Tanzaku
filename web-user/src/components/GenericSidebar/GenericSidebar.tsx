import { useState, useRef, useEffect, useCallback, type ReactNode } from 'react'
import { X } from 'lucide-react'
import './GenericSidebar.css'

interface Props {
  open: boolean
  title: string
  onClose: () => void
  children: ReactNode
  footer?: ReactNode
  minWidth?: number
  maxWidth?: number
  initialWidth?: number
}

export function GenericSidebar({
  open, title, onClose, children, footer,
  minWidth = 400, maxWidth = 800, initialWidth = 520,
}: Props) {
  const [width, setWidth] = useState(initialWidth)
  const [dragging, setDragging] = useState(false)
  const [visible, setVisible] = useState(false)
  const [mounted, setMounted] = useState(false)
  const startXRef = useRef(0)
  const startWidthRef = useRef(0)

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault()
    setDragging(true)
    startXRef.current = e.clientX
    startWidthRef.current = width
  }, [width])

  useEffect(() => {
    if (!dragging) return
    const onMouseMove = (e: MouseEvent) => {
      const delta = startXRef.current - e.clientX
      setWidth(Math.min(Math.max(startWidthRef.current + delta, minWidth), maxWidth))
    }
    const onMouseUp = () => setDragging(false)
    document.addEventListener('mousemove', onMouseMove)
    document.addEventListener('mouseup', onMouseUp)
    return () => {
      document.removeEventListener('mousemove', onMouseMove)
      document.removeEventListener('mouseup', onMouseUp)
    }
  }, [dragging, minWidth, maxWidth])

  useEffect(() => {
    if (open) {
      setVisible(false)
      setMounted(true)
      const raf = requestAnimationFrame(() => {
        requestAnimationFrame(() => setVisible(true))
      })
      return () => cancelAnimationFrame(raf)
    } else {
      setVisible(false)
      const timer = setTimeout(() => setMounted(false), 300)
      return () => clearTimeout(timer)
    }
  }, [open])

  const handleClose = useCallback(() => {
    setVisible(false)
    setTimeout(() => {
      setMounted(false)
      onClose()
    }, 300)
  }, [onClose])

  if (!mounted) return null

  return (
    <>
      <div
        className={`generic-sidebar__overlay ${visible ? 'generic-sidebar__overlay--visible' : ''}`}
        onClick={handleClose}
      />
      <div
        className={`generic-sidebar ${visible ? 'generic-sidebar--open' : ''}`}
        style={{ width: `${width}px` }}
      >
        <div
          className={`generic-sidebar__resize ${dragging ? 'generic-sidebar__resize--active' : ''}`}
          onMouseDown={onMouseDown}
        />
        <div className="generic-sidebar__header">
          <span className="generic-sidebar__title">{title}</span>
          <button className="generic-sidebar__close" onClick={handleClose}>
            <X size={18} />
          </button>
        </div>
        <div className="generic-sidebar__body">
          {children}
        </div>
        {footer && (
          <div className="generic-sidebar__footer">
            {footer}
          </div>
        )}
      </div>
    </>
  )
}
