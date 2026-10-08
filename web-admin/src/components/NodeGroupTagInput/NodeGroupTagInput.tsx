import { useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { X } from 'lucide-react'
import './NodeGroupTagInput.css'

export type NodeGroupOption = { id: string; name: string; enabled?: boolean }

function normalizeName(value: string): string {
  return value.trim().replace(/\s+/g, ' ')
}

export function NodeGroupTagInput({
  value,
  onChange,
  suggestions,
  disabled,
  onDraftChange,
}: {
  value: string[]
  onChange: (names: string[]) => void
  suggestions: NodeGroupOption[]
  disabled?: boolean
  onDraftChange?: (draft: string) => void
}) {
  const { t } = useTranslation()
  const [draft, setDraft] = useState('')
  const [activeIndex, setActiveIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  const selectedSet = useMemo(() => new Set(value.map((name) => name.toLowerCase())), [value])

  const filtered = useMemo(() => {
    const q = normalizeName(draft).toLowerCase()
    return suggestions
      .filter((item) => item.enabled !== false)
      .filter((item) => !selectedSet.has(item.name.toLowerCase()))
      .filter((item) => !q || item.name.toLowerCase().includes(q))
      .slice(0, 12)
  }, [draft, selectedSet, suggestions])

  function addName(raw: string) {
    const name = normalizeName(raw)
    if (!name || name.length > 100) return
    const exists = value.some((item) => item.toLowerCase() === name.toLowerCase())
    if (exists) {
      setDraft('')
      return
    }
    onChange([...value, name])
    setDraft('')
    setActiveIndex(0)
  }

  function removeName(name: string) {
    onChange(value.filter((item) => item !== name))
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === 'Enter') {
      event.preventDefault()
      if (filtered.length > 0 && draft.trim()) {
        addName(filtered[Math.min(activeIndex, filtered.length - 1)]!.name)
        return
      }
      addName(draft)
      return
    }
    if (event.key === 'Backspace' && !draft && value.length > 0) {
      onChange(value.slice(0, -1))
      return
    }
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveIndex((current) => Math.min(current + 1, Math.max(filtered.length - 1, 0)))
    }
    if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveIndex((current) => Math.max(current - 1, 0))
    }
  }

  return (
    <div className="node-group-tag-input">
      <div
        className="node-group-tag-input__box"
        onClick={() => inputRef.current?.focus()}
        role="group"
        aria-label={t('nodeGroup.tagInputLabel')}
      >
        {value.map((name) => (
          <span key={name} className="node-group-tag-input__tag">
            {name}
            {!disabled ? (
              <button
                type="button"
                className="node-group-tag-input__tag-remove"
                aria-label={t('nodeGroup.removeTag', { name })}
                onClick={(event) => {
                  event.stopPropagation()
                  removeName(name)
                }}
              >
                <X size={14} />
              </button>
            ) : null}
          </span>
        ))}
        <input
          ref={inputRef}
          className="node-group-tag-input__field"
          value={draft}
          disabled={disabled}
          placeholder={value.length ? t('nodeGroup.tagInputPlaceholderMore') : t('nodeGroup.tagInputPlaceholder')}
          onChange={(event) => {
            const next = event.target.value
            setDraft(next)
            setActiveIndex(0)
            onDraftChange?.(next)
          }}
          onKeyDown={onKeyDown}
        />
      </div>
      {filtered.length > 0 && draft.trim() ? (
        <div className="node-group-tag-input__suggestions" role="listbox">
          {filtered.map((item, index) => (
            <button
              key={item.id}
              type="button"
              role="option"
              className={`node-group-tag-input__suggestion${index === activeIndex ? ' node-group-tag-input__suggestion--active' : ''}`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => addName(item.name)}
            >
              {item.name}
            </button>
          ))}
        </div>
      ) : null}
      <p className="node-group-tag-input__hint">{t('nodeGroup.tagInputHint')}</p>
    </div>
  )
}
