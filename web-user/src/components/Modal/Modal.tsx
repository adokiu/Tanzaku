import { useEffect, useRef, useState, type ReactNode } from 'react'
import { X, AlertTriangle } from 'lucide-react'
import { Button } from '@/components/Button/Button'
import './Modal.css'

interface ModalProps {
  open: boolean
  onClose: () => void
  title: string
  children: ReactNode
  footer?: ReactNode
  width?: number
  // 确认弹窗模式
  confirmMode?: boolean
  confirmText?: string
  cancelText?: string
  confirmVariant?: 'danger' | 'primary'
  onConfirm?: () => void
  requireInput?: boolean
  requireInputLabel?: string
  requireInputValue?: string
  requireInputPlaceholder?: string
}

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  width = 520,
  confirmMode = false,
  confirmText = '确认',
  cancelText = '取消',
  confirmVariant = 'danger',
  onConfirm,
  requireInput = false,
  requireInputLabel,
  requireInputValue = '',
  requireInputPlaceholder,
}: ModalProps) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const [inputValue, setInputValue] = useState('')

  useEffect(() => {
    if (open) setInputValue('')
  }, [open])

  useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [open, onClose])

  useEffect(() => {
    if (open) {
      document.body.style.overflow = 'hidden'
      requestAnimationFrame(() => {
        overlayRef.current?.classList.add('modal-overlay--visible')
        panelRef.current?.classList.add('modal-panel--visible')
      })
    }
    return () => {
      document.body.style.overflow = ''
    }
  }, [open])

  if (!open) return null

  const handleOverlayClick = (e: React.MouseEvent) => {
    if (e.target === overlayRef.current) onClose()
  }

  const canConfirm = !requireInput || inputValue === requireInputValue

  return (
    <div ref={overlayRef} className="modal-overlay" onClick={handleOverlayClick}>
      <div ref={panelRef} className="modal-panel" style={{ maxWidth: width }}>
        <div className="modal-header">
          <h2 className="modal-title">{title}</h2>
          <button className="modal-close" onClick={onClose} aria-label="关闭">
            <X size={18} />
          </button>
        </div>
        <div className="modal-body">
          {confirmMode && (
            <div className="modal-confirm__body">
              <div className={`modal-confirm__icon modal-confirm__icon--${confirmVariant}`}>
                <AlertTriangle size={24} />
              </div>
              <div className="modal-confirm__message">{children}</div>
              {requireInput && (
                <div className="modal-confirm__input-section">
                  {requireInputLabel && (
                    <label className="modal-confirm__label">
                      {requireInputLabel} <span className="modal-confirm__required">「{requireInputValue}」</span>
                    </label>
                  )}
                  <input
                    className="modal-confirm__input"
                    value={inputValue}
                    onChange={(e) => setInputValue(e.target.value)}
                    placeholder={requireInputPlaceholder || `请输入 ${requireInputValue}`}
                    autoFocus
                  />
                </div>
              )}
            </div>
          )}
          {!confirmMode && children}
        </div>
        {confirmMode ? (
          <div className="modal-footer">
            <Button variant="ghost" onClick={onClose}>{cancelText}</Button>
            <Button
              variant={confirmVariant}
              onClick={onConfirm}
              disabled={!canConfirm}
            >
              {confirmText}
            </Button>
          </div>
        ) : (
          footer && <div className="modal-footer">{footer}</div>
        )}
      </div>
    </div>
  )
}
